param(
    [Parameter(Mandatory)]
    [string] $HostExecutable
)

$ErrorActionPreference = 'Stop'
$program = Get-Item -LiteralPath $HostExecutable
if ($program.Name -ne 'tickmark-browser-host.exe' -or $program.PSIsContainer) {
    throw 'Select the Tickmark browser host executable.'
}

$extension = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../native/edge-extension/manifest.json'), [Text.Encoding]::UTF8) | ConvertFrom-Json
$publicKey = [Convert]::FromBase64String($extension.key)
$sha = [Security.Cryptography.SHA256]::Create()
try {
    $hash = $sha.ComputeHash($publicKey)
} finally {
    $sha.Dispose()
}
$id = -join $(foreach ($byte in $hash[0..15]) {
    [char](97 + ($byte -shr 4))
    [char](97 + ($byte -band 15))
})

$name = 'com.tickmark.edge'
$manifestPath = Join-Path $program.DirectoryName "$name.json"
$manifest = @{
    name = $name
    description = 'Tickmark Edge foreground page bridge'
    path = $program.FullName
    type = 'stdio'
    allowed_origins = @("chrome-extension://$id/")
}
[IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Software\Microsoft\Edge\NativeMessagingHosts\$name")
try {
    $key.SetValue('', $manifestPath, [Microsoft.Win32.RegistryValueKind]::String)
} finally {
    $key.Dispose()
}
Write-Output $id
