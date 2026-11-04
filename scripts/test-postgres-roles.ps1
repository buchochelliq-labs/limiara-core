$ErrorActionPreference = "Stop"

$repositoryRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repositoryRoot

$project = "limiara-postgres-roles-$PID"
$env:POSTGRES_PORT = "0"

function Invoke-Compose {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments)
    & docker compose @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "docker compose failed with exit code $LASTEXITCODE"
    }
}

try {
    Invoke-Compose config --quiet
    Invoke-Compose --project-name $project up --detach --wait postgres
    Invoke-Compose --project-name $project run --rm migrate

    Get-Content -Raw -LiteralPath "tests/integration/postgres_role_reprovision_fixture.sql" |
        & docker compose --project-name $project exec --no-TTY postgres `
            psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1
    if ($LASTEXITCODE -ne 0) {
        throw "PostgreSQL role reprovision fixture failed with exit code $LASTEXITCODE"
    }

    Invoke-Compose --project-name $project exec --no-TTY postgres bash /docker-entrypoint-initdb.d/010_roles.sh

    Invoke-Compose --project-name $project exec --no-TTY postgres bash -ceu `
        'export PGPASSWORD="$PLATFORM_MIGRATOR_PASSWORD"; psql --host=127.0.0.1 --username=platform_migrator --dbname=platform --command="SELECT 1" >/dev/null'
    Invoke-Compose --project-name $project exec --no-TTY postgres bash -ceu `
        'export PGPASSWORD="$PLATFORM_APP_PASSWORD"; psql --host=127.0.0.1 --username=platform_app --dbname=platform --command="SELECT 1" >/dev/null'
    Invoke-Compose --project-name $project exec --no-TTY postgres bash -ceu `
        'export PGPASSWORD="$PLATFORM_WORKER_PASSWORD"; psql --host=127.0.0.1 --username=platform_worker --dbname=platform --command="SELECT 1" >/dev/null'

    Get-Content -Raw -LiteralPath "tests/integration/postgres_roles.sql" |
        & docker compose --project-name $project exec --no-TTY postgres `
            psql --username platform_bootstrap --dbname platform --set=ON_ERROR_STOP=1
    if ($LASTEXITCODE -ne 0) {
        throw "PostgreSQL role boundary assertions failed with exit code $LASTEXITCODE"
    }
}
catch {
    & docker compose --project-name $project ps --all
    & docker compose --project-name $project logs --no-color postgres
    throw
}
finally {
    & docker compose --project-name $project down --volumes --remove-orphans | Out-Null
}
