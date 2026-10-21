param([Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
$taskContainer = docker compose -p yydbmanager-validation -f apps/yydbmanager/tests/docker/compose.yml ps -q db2
if ($LASTEXITCODE -ne 0 -or !$taskContainer) { throw 'Start the isolated Db2 service first.' }
$taskDeadline = [DateTime]::UtcNow.AddMinutes(10)
do {
    docker exec $taskContainer su - db2inst1 -c 'db2 connect to YYTEST' 2>&1 | Out-Null
    if ($LASTEXITCODE -eq 0) { break }
    if ([DateTime]::UtcNow -gt $taskDeadline) { throw 'Db2 initialization timed out.' }
    Start-Sleep -Seconds 5
} while ($true)
# Only the disposable test instance profile is used; user installations are untouched.
docker exec $taskContainer tar --exclude=home/db2inst1/sqllib/db2dump -czf /tmp/yydb-client-instance.tgz -C / home/db2inst1/sqllib var/db2
if ($LASTEXITCODE -ne 0) { throw 'Cannot prepare the Db2 instance profile.' }
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
docker cp "${taskContainer}:/tmp/yydb-client-instance.tgz" (Join-Path $OutputDirectory 'db2-instance.tgz')
if ($LASTEXITCODE -ne 0) { throw 'Cannot copy the Db2 instance profile.' }
