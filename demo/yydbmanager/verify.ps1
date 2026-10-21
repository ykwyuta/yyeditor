param(
    [ValidateSet('postgres','mysql','sqlserver','oracle','db2')]
    [string[]]$Services=@('postgres')
)
$ErrorActionPreference='Stop'
$taskRoot=Split-Path -Parent $PSCommandPath
$taskCompose=@('compose','-p','yydbmanager-demo','-f',(Join-Path $taskRoot 'compose.yaml'),'--profile','*')
$taskExpected=@{}
(Get-Content (Join-Path $taskRoot 'data/expected-counts.json') -Raw | ConvertFrom-Json).PSObject.Properties | ForEach-Object {$taskExpected[$_.Name]=$_.Value}
$taskReport=[Collections.Generic.List[object]]::new()
function Invoke-DemoDocker([string[]]$Arguments, [switch]$AllowNoRows) {
    $taskOutput=& docker @taskCompose @Arguments
    # Db2 CLP returns 1 for a successful SELECT with no rows (e.g. no lock waits).
    if($LASTEXITCODE -ne 0 -and !($AllowNoRows -and $LASTEXITCODE -eq 1)){throw "Docker command failed: $($Arguments -join ' ')`n$($taskOutput -join "`n")"}
    $taskOutput
}
foreach($taskService in $Services) {
    $taskId=Invoke-DemoDocker @('ps','-q',$taskService)
    if(!$taskId){throw "$taskService is not running; start its Compose profile first"}
    $taskHealth=& docker inspect --format '{{.State.Health.Status}}' $taskId
    if($LASTEXITCODE -ne 0 -or $taskHealth -ne 'healthy'){throw "$taskService is not ready ($taskHealth); inspect docker compose logs"}
    $taskArguments=switch($taskService) {
        'postgres' { @('exec','-T','postgres','psql','-v','ON_ERROR_STOP=1','-h','127.0.0.1','-U','yy_demo','-d','yydemo','-tA','-F','|','-f','/demo/sql/00-verify-counts.sql') }
        'mysql' { @('exec','-T','-e','MYSQL_PWD=YyDemo_2026!','mysql','mysql','-h','127.0.0.1','-u','yy_demo','-D','yydemo','-NB','-e','source /demo/sql/00-verify-counts.sql') }
        'sqlserver' { @('exec','-T','sqlserver','/opt/mssql-tools18/bin/sqlcmd','-S','127.0.0.1','-U','yy_demo','-C','-b','-d','yydemo','-h','-1','-W','-s','|','-i','/demo/sql/00-verify-counts.sql') }
        'oracle' { @('exec','-T','-e','NLS_LANG=.AL32UTF8','oracle','sqlplus','-L','-s','yy_demo/"YyDemo_2026!"@//127.0.0.1:1521/FREEPDB1','@/demo/sql/verify-oracle.sql') }
        'db2' { @('exec','-T','db2','su','-','db2inst1','-c','db2 connect to YYDEMO >/dev/null && db2 -stf /demo/sql/00-verify-counts.sql') }
    }
    $taskLines=Invoke-DemoDocker $taskArguments
    $taskSeen=@{}
    foreach($taskLine in $taskLines) {
        if($taskLine -match '^\s*([a-z_]+)[|\t ]+(\d+)[|\t ]+(\d+)\s*$') {
            $taskTable=$Matches[1];$taskExpectedValue=[long]$Matches[2];$taskActual=[long]$Matches[3]
            if(!$taskExpected.ContainsKey($taskTable) -or $taskExpectedValue -ne $taskExpected[$taskTable] -or $taskActual -ne $taskExpected[$taskTable]) {throw "$taskService / $taskTable count mismatch: $taskLine"}
            if($taskSeen.ContainsKey($taskTable)){throw "Duplicate verification result: $taskTable"}
            $taskSeen[$taskTable]=$taskActual
        }
    }
    if($taskSeen.Count -ne $taskExpected.Count){throw "Incomplete verification for $taskService ($($taskSeen.Count)/$($taskExpected.Count)): $($taskLines -join '; ')"}
    $taskSamples=@('01-overview.sql','02-join-and-plan.sql','03-index-selectivity.sql','04-exact-values-and-logical-relation.sql',"performance/$taskService.sql")
    if($taskService -eq 'oracle') {
        Invoke-DemoDocker @($taskArguments | ForEach-Object {$_ -replace 'verify-oracle.sql','verify-oracle-samples.sql'}) | Out-Null
    } else {
        foreach($taskSample in $taskSamples) {
            $taskSampleArguments=@($taskArguments | ForEach-Object {$_ -replace '00-verify-counts.sql',$taskSample})
            $taskSampleOutput=Invoke-DemoDocker $taskSampleArguments -AllowNoRows:($taskService -eq 'db2')
            if($taskSampleOutput -match '(?m)^ERROR |^ORA-\d|SQL\d{4}N') {throw "$taskService sample failed: $taskSample"}
        }
    }
    $taskImage=& docker inspect --format '{{.Image}}' $taskId
    if($LASTEXITCODE -ne 0){throw 'Could not inspect image'}
    $taskReport.Add([ordered]@{service=$taskService;status='passed';image=$taskImage;counts=$taskSeen;samples=$taskSamples})
    Write-Host "$taskService : $($taskSeen.Count) table counts and $($taskSamples.Count) sample files OK"
}
$taskReportPath=Join-Path $taskRoot 'reports'
New-Item -ItemType Directory -Force $taskReportPath | Out-Null
[ordered]@{verified_at_utc=[DateTime]::UtcNow.ToString('o');databases=$taskReport} | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $taskReportPath 'verification.json') -Encoding utf8
