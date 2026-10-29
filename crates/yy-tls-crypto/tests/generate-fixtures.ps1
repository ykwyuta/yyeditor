param(
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$OpenSSL = 'openssl'
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
function Invoke-TlsOpenSSL([string[]]$Arguments) {
    & $OpenSSL @Arguments
    if ($LASTEXITCODE -ne 0) { throw "OpenSSL failed: $($Arguments -join ' ')" }
}
Invoke-TlsOpenSSL @('req','-x509','-newkey','rsa:2048','-nodes','-sha256','-days','36500',
    '-subj','/CN=yyeditor TLS TEST ONLY CA','-keyout',"$OutputDirectory/ca-key.pem",'-out',"$OutputDirectory/ca.pem",
    '-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign')
@'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth,clientAuth
subjectAltName=DNS:localhost,IP:127.0.0.1
'@ | Set-Content -Encoding ascii -LiteralPath "$OutputDirectory/extensions.cnf"
foreach ($kind in @('rsa','p256','p384','ed25519')) {
    $key = "$OutputDirectory/$kind-key.pem"
    $keyArguments = switch ($kind) {
        'rsa' { @('genpkey','-algorithm','RSA','-pkeyopt','rsa_keygen_bits:2048') }
        'p256' { @('genpkey','-algorithm','EC','-pkeyopt','ec_paramgen_curve:P-256') }
        'p384' { @('genpkey','-algorithm','EC','-pkeyopt','ec_paramgen_curve:P-384') }
        'ed25519' { @('genpkey','-algorithm','ED25519') }
    }
    Invoke-TlsOpenSSL ($keyArguments + @('-out',$key))
    Invoke-TlsOpenSSL @('req','-new','-key',$key,'-subj',"/CN=localhost",'-out',"$OutputDirectory/$kind.csr")
    Invoke-TlsOpenSSL @('x509','-req','-in',"$OutputDirectory/$kind.csr",'-CA',"$OutputDirectory/ca.pem",
        '-CAkey',"$OutputDirectory/ca-key.pem",'-set_serial',([string](10 + [array]::IndexOf(@('rsa','p256','p384','ed25519'),$kind))),
        '-days','36500','-sha256','-extfile',"$OutputDirectory/extensions.cnf",'-out',"$OutputDirectory/$kind.pem")
}
Invoke-TlsOpenSSL @('rsa','-in',"$OutputDirectory/rsa-key.pem",'-traditional','-out',"$OutputDirectory/rsa-pkcs1.pem")
Invoke-TlsOpenSSL @('ec','-in',"$OutputDirectory/p256-key.pem",'-out',"$OutputDirectory/p256-sec1.pem")
Invoke-TlsOpenSSL @('ec','-in',"$OutputDirectory/p384-key.pem",'-out',"$OutputDirectory/p384-sec1.pem")
Invoke-TlsOpenSSL @('req','-x509','-newkey','rsa:1024','-nodes','-sha256','-days','36500',
    '-subj','/CN=weak RSA TEST ONLY','-keyout',"$OutputDirectory/weak-rsa-key.pem",'-out',"$OutputDirectory/weak-rsa.pem")
