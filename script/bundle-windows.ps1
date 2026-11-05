[CmdletBinding()]
Param(
    [Parameter()][Alias('v')][string]$AppVersion,
    [Parameter()][string]$Name
)

$buildSuccess = $false

$AppWorkspaceDir = (Get-Location).Path
$AppMainCrate = "remind_me"
$AppCrateDir = "$AppWorkspaceDir\crates\$AppMainCrate"
$InnoResDir = "$AppWorkspaceDir\.inno"
$InnoOutDir = "$AppWorkspaceDir\Packages\windows"

$AppName = "RemindMe"

$CargoOutDir = "$AppWorkspaceDir\target\release"
$AppExeName = "$AppName.exe"

$InnoOutBaseName = "$AppName`_v$AppVersion`_Setup"

if (-not $AppVersion)
{
    throw "Invalid AppVersion: Please add -v to provide a valid AppVersion"
}

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
        "AppName"        = $AppName
        "AppVersion"     = $AppVersion
        "AppPublisher"   = "Yuan Tee"
        "AppURL"         = "https://github.com/YBKF/remind-me"
        "AppExeName"     = "$AppExeName"
        "WorkspaceDir"   = "$AppWorkspaceDir"
        "InnoOutDir"     = "$InnoOutDir"
        "InnoOutBaseName"= $InnoOutBaseName
        "ResourceDir"    = "$InnoResDir"
    }

    $defs = @()
    foreach ($key in $definitions.Keys)
    {
        $defs += "/d$key=`"$($definitions[$key])`""
    }

    $innoArgs = @($issFilePath) + $defs

    Write-Host "Running Inno Setup: $innoSetupPath $innoArgs"
    $process = Start-Process -FilePath $innoSetupPath -ArgumentList $innoArgs -NoNewWindow -Wait -PassThru

    if ($process.ExitCode -eq 0)
    {
        $script:buildSuccess = $true
    } else
    {
        $script:buildSuccess = $false
    }
}

# 生成安装包的签名
function SignInstaller
{
    $minisignPath = "C:\Scoop\shims\minisign.exe"

    Write-Host "Signing Installer"

    & $minisignPath -S -m "$InnoOutDir\$InnoOutBaseName.exe"
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

PrepareForBundle
BuildAppAndItsFriends
CollectFiles
BuildInstaller

if ($buildSuccess)
{
    Write-Output "Build successful"
} else
{
    Write-Output "Build failed"
    exit 1
}

SignInstaller

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
