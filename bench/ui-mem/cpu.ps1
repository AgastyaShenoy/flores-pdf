# CPU time an app spends opening a PDF, sitting idle, paging through it and zooming, read from
# Process.TotalProcessorTime (all threads, user + kernel). Keys go to the window as messages, so
# focus doesn't matter. Use a portable Sumatra copy (see measure.ps1).
# usage: powershell -File cpu.ps1 -Sumatra <portable SumatraPDF.exe> -Pdf <file.pdf> [-Runs 3]
param(
    [Parameter(Mandatory)] [string]$Sumatra,
    [Parameter(Mandatory)] [string]$Pdf,
    [int]$Runs = 3,
    [string]$Reader = (Join-Path $PSScriptRoot '..\..\target\release\pdfreader.exe'),
    [string[]]$Apps = @('sumatra', 'pdfreader')
)

Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Win {
  [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern uint MapVirtualKey(uint code, uint type);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr p);
  [StructLayout(LayoutKind.Sequential)] struct RECT { public int L, T, R, B; }
  // the process's largest visible window; Process.MainWindowHandle can be a hidden helper
  public static IntPtr Find(int pid) {
    IntPtr best = IntPtr.Zero; long bestArea = 0;
    EnumWindows((h, p) => {
      uint owner; GetWindowThreadProcessId(h, out owner); RECT r;
      if (owner == pid && IsWindowVisible(h) && GetWindowRect(h, out r)) {
        long a = (long)(r.R - r.L) * (r.B - r.T);
        if (a > bestArea) { bestArea = a; best = h; }
      }
      return true;
    }, IntPtr.Zero);
    return best;
  }
  // key-down, then key-up 100 ms later so the WM_CHAR that TranslateMessage queues comes first
  public static void Key(IntPtr h, int vk) {
    int sc = (int)MapVirtualKey((uint)vk, 0) << 16;
    if (vk >= 0x21 && vk <= 0x28) sc |= 0x01000000;
    PostMessage(h, 0x0100, (IntPtr)vk, (IntPtr)(sc | 1));
    System.Threading.Thread.Sleep(100);
    PostMessage(h, 0x0101, (IntPtr)vk, (IntPtr)unchecked((int)(0xC0000001 | (uint)sc)));
  }
}
'@

$PAGE_DOWN = 0x22; $PLUS = 0x6B; $MINUS = 0x6D   # numpad + and - type the same character in both apps

function Measure-Once($exe) {
    $p = Start-Process -FilePath $exe -ArgumentList "`"$Pdf`"" -PassThru
    $cpu = { $p.Refresh(); $p.TotalProcessorTime.TotalSeconds }
    $r = [ordered]@{}
    Start-Sleep -Seconds 6
    $h = [Win]::Find($p.Id)
    $r.open_s = & $cpu
    $t = & $cpu; Start-Sleep -Seconds 10; $r.idle_10s = (& $cpu) - $t
    $t = & $cpu
    1..20 | ForEach-Object { [Win]::Key($h, $PAGE_DOWN); Start-Sleep -Milliseconds 100 }
    Start-Sleep -Seconds 5; $r.page_down_x20 = (& $cpu) - $t
    $t = & $cpu
    foreach ($k in $PLUS, $PLUS, $PLUS, $MINUS, $MINUS, $MINUS) { [Win]::Key($h, $k); Start-Sleep -Milliseconds 400 }
    Start-Sleep -Seconds 5; $r.zoom_in3_out3 = (& $cpu) - $t
    $r.title = $p.MainWindowTitle
    Stop-Process -Id $p.Id -Force; Start-Sleep -Milliseconds 500
    [pscustomobject]$r
}

foreach ($app in @(@{ Name = 'sumatra'; Exe = $Sumatra }, @{ Name = 'pdfreader'; Exe = $Reader }) | Where-Object { $Apps -contains $_.Name }) {
    # not $runs: PowerShell names ignore case, so that would overwrite $Runs
    $results = 1..$Runs | ForEach-Object { Measure-Once $app.Exe }
    $median = { param($f) ($results | ForEach-Object $f | Sort-Object)[[math]::Floor($Runs / 2)] }
    [pscustomobject]@{
        app = $app.Name
        file = Split-Path $Pdf -Leaf
        open_cpu_s = [math]::Round((& $median 'open_s'), 2)
        idle_10s_cpu_s = [math]::Round((& $median 'idle_10s'), 3)
        page_down_x20_cpu_s = [math]::Round((& $median 'page_down_x20'), 2)
        zoom_cpu_s = [math]::Round((& $median 'zoom_in3_out3'), 2)
        last_title = $results[-1].title
    } | ConvertTo-Json -Compress
}
