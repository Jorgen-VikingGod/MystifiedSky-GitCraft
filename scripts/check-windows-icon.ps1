param([Parameter(Mandatory = $true)][string]$Executable)

$ErrorActionPreference = 'Stop'
$path = (Resolve-Path -LiteralPath $Executable).ProviderPath
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class GitCraftShellIcons {
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern uint ExtractIconEx(string path, int index, IntPtr large, IntPtr small, uint count);
}
'@
# Index -1 with no output arrays asks the Windows shell for the icon group count.
$groups = [GitCraftShellIcons]::ExtractIconEx($path, -1, [IntPtr]::Zero, [IntPtr]::Zero, 0)
if ($groups -eq 0 -or $groups -eq [uint32]::MaxValue) {
    throw "Windows could not find an embedded icon in $path."
}
Write-Output "Verified $groups embedded icon group(s) in $path."
