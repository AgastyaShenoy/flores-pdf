# Builds the engine for Android (libflores.so, from crates/android) and copies it into the app's jniLibs.
#   powershell -File android/build-rust.ps1             # phones (arm64) and the emulator (x86_64)
#   powershell -File android/build-rust.ps1 -Abi arm64  # phones only
param([ValidateSet('all', 'arm64', 'x86_64')][string]$Abi = 'all')
$ErrorActionPreference = 'Stop'

$sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } else { "$env:LOCALAPPDATA\Android\Sdk" }
$ndk = Get-ChildItem "$sdk\ndk" -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
if (-not $ndk) { throw "No Android NDK found under $sdk\ndk" }
$bin = "$($ndk.FullName)\toolchains\llvm\prebuilt\windows-x86_64\bin"

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$jni = Join-Path $root 'android\app\src\main\jniLibs'
$targets = [ordered]@{ 'aarch64-linux-android' = 'arm64-v8a'; 'x86_64-linux-android' = 'x86_64' }
if ($Abi -eq 'arm64') { $targets.Remove('x86_64-linux-android') }
if ($Abi -eq 'x86_64') { $targets.Remove('aarch64-linux-android') }

# Android 8 and later, as the app's minSdk
$api = 26
# a smaller library, without symbols
$env:CARGO_PROFILE_RELEASE_STRIP = 'symbols'
foreach ($t in $targets.Keys) {
    $u = $t.Replace('-', '_')
    Set-Item "env:CARGO_TARGET_$($u.ToUpper())_LINKER" "$bin\$t$api-clang.cmd"
    # for any C a dependency builds
    Set-Item "env:CC_$u" "$bin\$t$api-clang.cmd"
    Set-Item "env:AR_$u" "$bin\llvm-ar.exe"
    # cargo reports progress on stderr; don't let PowerShell take that for a failure
    $ErrorActionPreference = 'Continue'
    & cargo build --release -p flores-android --target $t --manifest-path "$root\Cargo.toml" 2>&1 | ForEach-Object { "$_" }
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed for $t" }
    $dir = Join-Path $jni $targets[$t]
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item "$root\target\$t\release\libflores.so" "$dir\libflores.so" -Force
    '{0,-12} {1:N0} KB' -f $targets[$t], ((Get-Item "$dir\libflores.so").Length / 1KB)
}
