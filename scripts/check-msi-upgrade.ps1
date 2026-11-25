param(
    [Parameter(Mandatory)]
    [string]$Directory
)

$ErrorActionPreference = 'Stop'
$packages = @(Get-ChildItem -LiteralPath $Directory -Filter '*.msi' -File)
if ($packages.Count -ne 1) {
    throw "Expected exactly one MSI in $Directory. Found $($packages.Count)."
}

# Windows Installer identifies the released v0.1.43 and earlier MSI products by this ID.
$expected = '{7866E2C2-413E-59FC-9D43-5CB6AAAC29DB}'
$installer = New-Object -ComObject WindowsInstaller.Installer
$database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($packages[0].FullName, 0))

function Read-MsiValue([string]$Sql) {
    $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @($Sql))
    try {
        $null = $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null)
        $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
        if ($null -ne $record) {
            return $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, @(1))
        }
    } finally {
        $null = $view.GetType().InvokeMember('Close', 'InvokeMethod', $null, $view, $null)
    }
}

$actual = Read-MsiValue "SELECT Value FROM Property WHERE Property = 'UpgradeCode'"
if ($actual -ne $expected) {
    throw "MSI UpgradeCode is $actual. Existing GitCraft installations require $expected."
}
$upgrade = Read-MsiValue "SELECT UpgradeCode FROM Upgrade WHERE ActionProperty = 'WIX_UPGRADE_DETECTED'"
$remove = Read-MsiValue "SELECT Action FROM InstallExecuteSequence WHERE Action = 'RemoveExistingProducts'"
if ($upgrade -ne $expected -or $remove -ne 'RemoveExistingProducts') {
    throw 'The MSI must detect and remove an earlier GitCraft MSI during upgrade.'
}
Write-Output "Verified the GitCraft MSI UpgradeCode and removal of the previous product."
