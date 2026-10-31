# Starts each app, lets it settle, and records its memory the way Task Manager reports it.
# usage: powershell -File measure.ps1 -Sumatra <portable SumatraPDF.exe> -Pdf <file.pdf> [-Runs 3] [-Only 'egui*'] [-Scroll 150]
#
# -Scroll N sends N mouse wheel notches down after settling, then waits again before measuring,
# to see what the page caches grow to. The messages go straight to the window, so focus doesn't matter.
#
# The Sumatra copy must sit in its own folder (not the install folder) next to a SumatraPDF-settings.txt
# containing the lines below. That makes it portable, so it neither restores nor saves your own session:
#   CheckForUpdates = false
#   RememberOpenedFiles = false
#   RestoreSession = false
#   WindowState = 1
#   WindowPos = 100 100 1266 977
param(
    [Parameter(Mandatory)] [string]$Sumatra,
    [Parameter(Mandatory)] [string]$Pdf,
    [int]$Runs = 3,
    [string]$Only = '*',
    [int]$SettleSeconds = 6,
    [int]$Scroll = 0,
    [string]$Reader = (Join-Path $PSScriptRoot '..\..\target\release\pdfreader.exe')
)

Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Wheel {
  [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [StructLayout(LayoutKind.Sequential)] struct RECT { public int L, T, R, B; }
  delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr p);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  // The process's largest visible top-level window. Process.MainWindowHandle can be a hidden
  // helper window (it is for Sumatra), which ignores the wheel.
  public static IntPtr Find(int pid) {
    IntPtr best = IntPtr.Zero; long bestArea = 0;
    EnumWindows((h, p) => {
      uint owner; GetWindowThreadProcessId(h, out owner);
      RECT r;
      if (owner == pid && IsWindowVisible(h) && GetWindowRect(h, out r)) {
        long area = (long)(r.R - r.L) * (r.B - r.T);
        if (area > bestArea) { bestArea = area; best = h; }
      }
      return true;
    }, IntPtr.Zero);
    return best;
  }
  public static void Down(IntPtr h, int notches) {
    RECT r; GetWindowRect(h, out r);
    int x = (r.L + r.R) / 2, y = (r.T + r.B) / 2;
    for (int i = 0; i < notches; i++) {
      PostMessage(h, 0x020A, (IntPtr)(-120 << 16), (IntPtr)((y << 16) | (x & 0xFFFF)));
      System.Threading.Thread.Sleep(40);
    }
  }
}
'@

$bin = Join-Path $PSScriptRoot 'target\release'
$apps = @(
    @{ Name = 'pdfreader';         Exe = $Reader;                   Args = @() },
    @{ Name = 'pdfreader + pdf';   Exe = $Reader;                   Args = @($Pdf) },
    @{ Name = 'sumatra';           Exe = $Sumatra;                  Args = @() },
    @{ Name = 'sumatra + pdf';     Exe = $Sumatra;                  Args = @($Pdf) },
    @{ Name = 'gdi';               Exe = "$bin\gdi.exe";            Args = @() },
    @{ Name = 'gdi + pdf';         Exe = "$bin\gdi.exe";            Args = @($Pdf) },
    @{ Name = 'egui-cpu';          Exe = "$bin\egui-cpu.exe";       Args = @() },
    @{ Name = 'egui-cpu + pdf';    Exe = "$bin\egui-cpu.exe";       Args = @($Pdf) },
    @{ Name = 'wgpu-clear dx12';   Exe = "$bin\wgpu-clear.exe";     Args = @(); Env = @{ WGPU_BACKEND = 'dx12' } },
    @{ Name = 'wgpu-clear dx12 low-mem'; Exe = "$bin\wgpu-clear.exe"; Args = @(); Env = @{ WGPU_BACKEND = 'dx12'; MEM = 'low' } },
    @{ Name = 'wgpu-clear vulkan'; Exe = "$bin\wgpu-clear.exe";     Args = @(); Env = @{ WGPU_BACKEND = 'vulkan' } },
    @{ Name = 'egui-wgpu';         Exe = "$bin\egui-wgpu.exe";      Args = @() },
    @{ Name = 'egui-wgpu + pdf';   Exe = "$bin\egui-wgpu.exe";      Args = @($Pdf) },
    @{ Name = 'egui-wgpu low-mem'; Exe = "$bin\egui-wgpu.exe";      Args = @(); Env = @{ MEM = 'low' } },
    @{ Name = 'egui-wgpu + pdf low-mem'; Exe = "$bin\egui-wgpu.exe"; Args = @($Pdf); Env = @{ MEM = 'low' } },
    @{ Name = 'egui-glow';         Exe = "$bin\egui-glow.exe";      Args = @() },
    @{ Name = 'egui-glow + pdf';   Exe = "$bin\egui-glow.exe";      Args = @($Pdf) }
)

function Measure-Once($app) {
    $saved = @{}
    foreach ($k in $app.Env.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $app.Env[$k]) }
    $p = if ($app.Args.Count) { Start-Process -FilePath $app.Exe -ArgumentList ($app.Args | ForEach-Object { "`"$_`"" }) -PassThru } else { Start-Process -FilePath $app.Exe -PassThru }
    foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    Start-Sleep -Seconds $SettleSeconds
    if ($p.HasExited) { return [pscustomobject]@{ app = $app.Name; error = "exited with code $($p.ExitCode)" } }
    $p.Refresh()
    if ($Scroll -gt 0) {
        [Wheel]::Down([Wheel]::Find($p.Id), $Scroll)
        Start-Sleep -Seconds $SettleSeconds
        $p.Refresh()
    }
    # the instance with our pid, in case another process has the same name
    $ws = Get-Counter '\Process(*)\ID Process', '\Process(*)\Working Set - Private' -ErrorAction SilentlyContinue
    $inst = ($ws.CounterSamples | Where-Object { $_.Path -like '*\id process' -and $_.CookedValue -eq $p.Id }).InstanceName
    $privWs = ($ws.CounterSamples | Where-Object { $_.Path -like '*working set - private' -and $_.InstanceName -eq $inst }).CookedValue
    $gpu = Get-Counter "\GPU Process Memory(pid_$($p.Id)_*)\Dedicated Usage", "\GPU Process Memory(pid_$($p.Id)_*)\Shared Usage" -ErrorAction SilentlyContinue
    $dedicated = ($gpu.CounterSamples | Where-Object { $_.Path -like '*dedicated usage' } | Measure-Object CookedValue -Sum).Sum
    $shared = ($gpu.CounterSamples | Where-Object { $_.Path -like '*shared usage' } | Measure-Object CookedValue -Sum).Sum
    $drivers = ($p.Modules | Where-Object { $_.ModuleName -match '^(nv|d3d12|d3d11|dxgi|vulkan|opengl32|d2d1)' } | ForEach-Object ModuleName | Sort-Object) -join ' '
    $r = [pscustomobject]@{
        app = if ($Scroll -gt 0) { "$($app.Name), scrolled $Scroll" } else { $app.Name }
        file = if ($app.Args.Count) { Split-Path $Pdf -Leaf } else { '' }
        private_ws_mb = [math]::Round($privWs / 1MB, 1)
        commit_mb = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
        total_ws_mb = [math]::Round($p.WorkingSet64 / 1MB, 1)
        gpu_dedicated_mb = [math]::Round($dedicated / 1MB, 1)
        gpu_shared_mb = [math]::Round($shared / 1MB, 1)
        modules = $p.Modules.Count
        gpu_dlls = $drivers
    }
    Stop-Process -Id $p.Id -Force
    Start-Sleep -Milliseconds 500
    $r
}

foreach ($app in $apps | Where-Object { $_.Name -like $Only }) {
    if (-not (Test-Path $app.Exe)) { Write-Warning "missing $($app.Exe)"; continue }
    $results = 1..$Runs | ForEach-Object { Measure-Once $app }
    # median run by private working set
    $results | Sort-Object private_ws_mb | Select-Object -Index ([math]::Floor($Runs / 2)) | ConvertTo-Json -Compress
}
