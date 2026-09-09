# Put this program's directory into the machine PATH, or take it out again.
#
# Why a script and not four lines of NSIS: NSIS reads strings up to a fixed length — 1024
# characters in the standard build — and a machine PATH is routinely longer than that. Read
# it there and write it back and you have silently truncated the PATH of the whole machine,
# which is the kind of damage an installer must never do.
#
# Why the registry directly and not [Environment]::SetEnvironmentVariable: that call writes
# the value back as REG_SZ. PATH is REG_EXPAND_SZ, and entries like %SystemRoot%\system32
# stop being expanded the moment its type changes. The value is read without expanding it
# and written back with its type intact.

param(
    [Parameter(Mandatory)][ValidateSet('add', 'remove')][string]$Action,
    [Parameter(Mandatory)][string]$Directory
)

$ErrorActionPreference = 'Stop'
$key = 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
$item = Get-Item -LiteralPath $key
$current = $item.GetValue('Path', '', 'DoNotExpandEnvironmentNames')

$wanted = $Directory.TrimEnd('\')
# Removed in both cases, so "add" cannot end up with two of them and "remove" takes the one
# that is there whether or not it was written with a trailing backslash.
$parts = @($current -split ';' | Where-Object { $_ -ne '' -and $_.TrimEnd('\') -ine $wanted })
if ($Action -eq 'add') { $parts += $Directory }

$new = $parts -join ';'
if ($new -eq $current) { exit 0 }
Set-ItemProperty -LiteralPath $key -Name 'Path' -Value $new -Type ExpandString

# Without this, a new command prompt still has the old PATH: it inherits its environment
# from Explorer, and Explorer only rereads on this message. Everything already open keeps
# the old one either way, which is worth saying to whoever is watching the installer.
$signature = @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam,
    string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
$user32 = Add-Type -MemberDefinition $signature -Name 'CyberbrainPath' -Namespace 'Win32' -PassThru
$result = [UIntPtr]::Zero
# HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG, two seconds. A window that does not
# answer is not a reason to fail an installation.
[void]$user32::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 2000, [ref]$result)
