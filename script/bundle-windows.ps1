$AppWorkspaceDir = (Get-Location).Path
$AppMainCrate = "remind_me"
$AppCrateDir = "$AppWorkspaceDir\crates\$AppMainCrate"
$InnoResDir = "$AppWorkspaceDir\.inno"
$InnoOutDir = "$AppWorkspaceDir\Packages\windows"

$MinisignKeyDir = Join-Path -Path $HOME -ChildPath ".minisign"
$MinisignKeyPath = Join-Path -Path $MinisignKeyDir -ChildPath "remind-me.key"

$AppName = "RemindMe"

$CargoOutDir = "$AppWorkspaceDir\target\release"
$AppExeName = "$AppName.exe"

# 版本号的唯一来源：主 crate 的 Cargo.toml。
# 用 cargo metadata 读取，可正确处理 version.workspace = true 这类继承写法。
function Get-AppVersion
{
    $metadataJson = cargo metadata --no-deps --format-version 1 --quiet

    if ($LASTEXITCODE -ne 0)
    {
        throw "cargo metadata failed with exit code $LASTEXITCODE"
    }

    $package = ($metadataJson -join "`n" | ConvertFrom-Json).packages |
        Where-Object { $_.name -eq $AppMainCrate } |
        Select-Object -First 1

    if (-not $package)
    {
        throw "Package '$AppMainCrate' not found in cargo metadata"
    }

    return $package.version
}

$AppVersion = Get-AppVersion
$InnoOutBaseName = "$AppName`_v$AppVersion`_Setup"

Write-Host "AppVersion: $AppVersion (from crates\$AppMainCrate\Cargo.toml)"

function PrepareForBundle
{
    if (Test-Path -Path "$InnoResDir")
    {
        Remove-Item -Path "$InnoResDir" -Recurse -Force
    }

    New-Item -Path "$InnoResDir" -ItemType Directory -Force | Out-Null
    New-Item -Path "$InnoResDir\tools" -ItemType Directory -Force | Out-Null
}

function BuildAppAndItsFriends
{
    Write-Output "Building App and its friends"

    cargo build --release --package $AppMainCrate --package auto_update_helper

    if ($LASTEXITCODE -ne 0)
    {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }

    Copy-Item -Path "$CargoOutDir\$AppExeName" -Destination "$InnoResDir\$AppExeName" -Force
    Copy-Item -Path "$CargoOutDir\auto_update_helper.exe" -Destination "$InnoResDir\auto_update_helper.exe" -Force
}

function CollectFiles
{
    Write-Output "Collecting files"

    Move-Item -Path "$InnoResDir\auto_update_helper.exe" -Destination "$InnoResDir\tools\auto_update_helper.exe" -Force
}

# 构建安装包
function BuildInstaller
{
    $issFilePath = "$AppCrateDir\assets\remind_me.iss"
    $innoSetupPath = "C:\Scoop\shims\iscc.exe"

    $definitions = @{
        "AppName"         = $AppName
        "AppVersion"      = $AppVersion
        "AppPublisher"    = "Yuan Tee"
        "AppURL"          = "https://github.com/YBKF/remind-me"
        "AppExeName"      = "$AppExeName"
        "WorkspaceDir"    = "$AppWorkspaceDir"
        "InnoOutDir"      = "$InnoOutDir"
        "InnoOutBaseName" = $InnoOutBaseName
        "ResourceDir"     = "$InnoResDir"
    }

    $defs = @()
    foreach ($key in $definitions.Keys)
    {
        $defs += "/d$key=`"$($definitions[$key])`""
    }

    $innoArgs = @($issFilePath) + $defs

    Write-Host "Running Inno Setup: $innoSetupPath $innoArgs"
    $process = Start-Process -FilePath $innoSetupPath -ArgumentList $innoArgs -NoNewWindow -Wait -PassThru

    if ($process.ExitCode -ne 0)
    {
        throw "Inno Setup failed with exit code $($process.ExitCode)"
    }
}

# 生成安装包的签名
function SignInstaller
{
    $minisignPath = "C:\Scoop\shims\minisign.exe"
    $installerPath = "$InnoOutDir\$InnoOutBaseName.exe"
    $sigPath = "$installerPath.minisig"

    Write-Host "Signing Installer"

    if (-not (Test-Path -Path $minisignPath))
    {
        throw "minisign not found: $minisignPath"
    }

    if (-not (Test-Path -Path $MinisignKeyPath))
    {
        throw "minisign secret key not found: $MinisignKeyPath"
    }

    if (-not (Test-Path -Path $installerPath))
    {
        throw "installer not found: $installerPath"
    }

    & $minisignPath -S -s "$MinisignKeyPath" -m "$installerPath"

    if ($LASTEXITCODE -ne 0)
    {
        throw "minisign failed with exit code $LASTEXITCODE"
    }

    if (-not (Test-Path -Path $sigPath))
    {
        throw "signature file not generated: $sigPath"
    }
}

# 生成版本信息
function GiteeCreateVersionJson
{
    $versionPath = "$AppWorkspaceDir\version"

    if (-not (Test-Path $versionPath))
    {
        New-Item -ItemType Directory -Path $versionPath | Out-Null
    }

    $versionJsonPath = "$versionPath\version.json"
    $sigPath = "$InnoOutDir\$InnoOutBaseName.exe.minisig"

    $denoScript = "$AppWorkspaceDir\script\gitee-create-version-json.ts"

    Write-Host "Running Deno script: $denoScript"

    # $null = suppresses deno's stdout so it doesn't pollute the function's return value
    $null = & deno run --allow-all $denoScript --sig-path "$sigPath" --output "$versionJsonPath"

    if ($LASTEXITCODE -eq 0)
    {
        return $true
    } else
    {
        return $false
    }
}

# 任一步骤出错即报错并终止脚本：Cmdlet 错误转为终止性错误，原生命令失败由各步骤内部 throw
$ErrorActionPreference = 'Stop'

try
{
    PrepareForBundle
    BuildAppAndItsFriends
    CollectFiles
    BuildInstaller
} catch
{
    Write-Host "Bundle failed: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

Write-Output "Build successful"


try
{
    SignInstaller
} catch
{
    Write-Host "Sign failed: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

$maxRetries = 3
$attempt = 0
$success = $false

while ($attempt -lt $maxRetries -and -not $success)
{
    Write-Host "`n========================================"
    Write-Host "Please upload the installer package to Gitee releases before proceeding."
    Write-Host "========================================"

    Write-Host "Press any key to continue..."
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")

    $attempt++
    Write-Host "`nAttempt $attempt of ${maxRetries}: Running GiteeCreateVersionJson..."
    $success = GiteeCreateVersionJson
    if (-not $success)
    {
        Write-Host "Attempt $attempt failed."
    }
}

if (-not $success)
{
    Write-Host "GiteeCreateVersionJson failed after $maxRetries attempts."
    exit 1
} else
{
    Write-Host "GiteeCreateVersionJson completed successfully."
}
