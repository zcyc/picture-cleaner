param(
    [Parameter(Mandatory = $true)]
    [string]$BundleRoot,
    [string]$CertificatePath,
    [string]$CertificatePassword,
    [string]$SignTool = "signtool.exe",
    [string]$TimestampUrl = "http://timestamp.digicert.com"
)

$signToolCommand = Get-Command $SignTool -ErrorAction SilentlyContinue
if ($null -eq $signToolCommand) {
    throw "signtool.exe not found; install the Windows SDK or set SIGNTOOL"
}

$files = @(Get-ChildItem -LiteralPath $BundleRoot -Recurse -File | Where-Object {
    $_.Extension -in ".exe", ".msi"
})
if ($files.Count -eq 0) {
    throw "No Windows installers found"
}

if ([string]::IsNullOrWhiteSpace($CertificatePath)) {
    $subject = "CN=Picture Cleaner Local"
    $certificate = Get-ChildItem Cert:\CurrentUser\My | Where-Object {
        $_.Subject -eq $subject -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date)
    } | Sort-Object NotAfter -Descending | Select-Object -First 1

    if ($null -eq $certificate) {
        $certificate = New-SelfSignedCertificate `
            -Type CodeSigningCert `
            -Subject $subject `
            -CertStoreLocation Cert:\CurrentUser\My `
            -KeyAlgorithm RSA `
            -KeyLength 2048 `
            -HashAlgorithm SHA256 `
            -NotAfter (Get-Date).AddYears(3)
    }

    $trustedCertificate = Join-Path $env:TEMP "picture-cleaner-local.cer"
    Export-Certificate -Cert $certificate -FilePath $trustedCertificate -Type CERT | Out-Null
    Import-Certificate -FilePath $trustedCertificate -CertStoreLocation Cert:\CurrentUser\TrustedPublisher | Out-Null
    Remove-Item -LiteralPath $trustedCertificate -Force -ErrorAction SilentlyContinue

    $signArguments = @(
        "sign", "/sha1", $certificate.Thumbprint,
        "/fd", "SHA256", "/tr", $TimestampUrl, "/td", "SHA256"
    )
    Write-Host "Using local self-signed certificate: $($certificate.Thumbprint)"
} else {
    if (-not (Test-Path -LiteralPath $CertificatePath -PathType Leaf)) {
        throw "Windows certificate not found: $CertificatePath"
    }

    $signArguments = @(
        "sign", "/fd", "SHA256", "/f", $CertificatePath,
        "/p", $CertificatePassword, "/tr", $TimestampUrl, "/td", "SHA256"
    )
}

foreach ($file in $files) {
    & $signToolCommand.Source @signArguments $file.FullName
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

foreach ($file in $files) {
    & $signToolCommand.Source verify /pa $file.FullName
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}
