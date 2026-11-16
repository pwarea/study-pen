[CmdletBinding()]
param(
    [switch]$SkipTests,
    [switch]$SourceOnly,
    [string]$GitHubRepository
)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$originalEncodedFlags = [Environment]::GetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', 'Process')
$originalUpdateRepository = [Environment]::GetEnvironmentVariable('STUDY_PEN_GITHUB_REPOSITORY', 'Process')

if ($GitHubRepository -and ($GitHubRepository -cnotmatch '\A[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?/[A-Za-z0-9_.-]{1,100}\z' -or
    ($GitHubRepository -split '/')[1] -in @('.', '..'))) {
    throw 'GitHubRepository must be the public repository name in owner/repository format.'
}

function Invoke-Cargo {
    param([string[]]$Arguments)
    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo failed with exit code $LASTEXITCODE."
    }
}

function Get-RemappingFlags {
    $flags = [Collections.Generic.List[string]]::new()
    $roots = [ordered]@{}
    $roots[$projectRoot] = '/study-pen'
    if ($env:CARGO_HOME) {
        $roots[[IO.Path]::GetFullPath($env:CARGO_HOME)] = '/cargo'
    }
    if ($env:RUSTUP_HOME) {
        $roots[[IO.Path]::GetFullPath($env:RUSTUP_HOME)] = '/rustup'
    }
    if ($env:USERPROFILE) {
        $roots[[IO.Path]::GetFullPath($env:USERPROFILE)] = '/user'
    }
    foreach ($entry in $roots.GetEnumerator()) {
        $flags.Add("--remap-path-prefix=$($entry.Key)=$($entry.Value)")
        $forwardSlashes = $entry.Key.Replace('\', '/')
        if ($forwardSlashes -ne $entry.Key) {
            $flags.Add("--remap-path-prefix=$forwardSlashes=$($entry.Value)")
        }
    }
    return $flags.ToArray()
}

function Write-DependencyNotices {
    param($Metadata, [string]$Destination)
    $builder = [Text.StringBuilder]::new()
    [void]$builder.AppendLine('Study Pen - third-party dependency notices')
    [void]$builder.AppendLine('The following packages retain their original licenses and notices.')
    [void]$builder.AppendLine('Build-time dependencies are included for completeness.')
    foreach ($package in ($Metadata.packages | Where-Object { $_.name -ne 'study-pen' } | Sort-Object name, version)) {
        [void]$builder.AppendLine()
        [void]$builder.AppendLine(('=' * 72))
        [void]$builder.AppendLine("$($package.name) $($package.version)")
        [void]$builder.AppendLine("License: $($package.license)")
        $packageDirectory = Split-Path -Parent $package.manifest_path
        $licenseFiles = @(Get-ChildItem -LiteralPath $packageDirectory -File | Where-Object {
            $_.Name -match '^(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE)([._-].*)?$'
        } | Sort-Object Name)
        if ($package.license_file) {
            $declaredLicense = Join-Path $packageDirectory $package.license_file
            if (Test-Path -LiteralPath $declaredLicense -PathType Leaf) {
                $licenseFiles += Get-Item -LiteralPath $declaredLicense
            }
        }
        $licenseFiles = @($licenseFiles | Sort-Object FullName -Unique)
        if ($licenseFiles.Count -eq 0) {
            throw "No license text was found for $($package.name). Add its required notice before packaging."
        }
        foreach ($file in $licenseFiles) {
            [void]$builder.AppendLine()
            [void]$builder.AppendLine($file.Name)
            [void]$builder.AppendLine([IO.File]::ReadAllText($file.FullName))
        }
    }
    [IO.File]::WriteAllText($Destination, $builder.ToString(), [Text.UTF8Encoding]::new($false))
}

function Remove-BuildTimestamp {
    param([string]$Executable)
    $bytes = [IO.File]::ReadAllBytes($Executable)
    if ($bytes.Length -lt 64 -or $bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a) {
        throw 'The release executable is not a valid PE file.'
    }
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
    if ($peOffset -lt 0 -or $peOffset + 12 -gt $bytes.Length -or
        [BitConverter]::ToUInt32($bytes, $peOffset) -ne 0x00004550) {
        throw 'The release executable has an invalid PE header.'
    }
    # The COFF timestamp is not required to run this unsigned portable executable.
    [Array]::Clear($bytes, $peOffset + 8, 4)
    [IO.File]::WriteAllBytes($Executable, $bytes)
}

function Assert-NoPrivateBuildPaths {
    param([string]$Executable)
    $bytes = [IO.File]::ReadAllBytes($Executable)
    $ascii = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
    foreach ($root in @($projectRoot, $env:USERPROFILE, $env:CARGO_HOME, $env:RUSTUP_HOME)) {
        if (-not $root) { continue }
        foreach ($spelling in @($root, $root.Replace('\', '/'))) {
            foreach ($encoding in @([Text.Encoding]::UTF8, [Text.Encoding]::Unicode)) {
                # Search raw encoded bytes, including UTF-16 strings at odd
                # offsets and user profiles containing non-ASCII characters.
                $needle = [Text.Encoding]::GetEncoding(28591).GetString($encoding.GetBytes($spelling))
                if ($ascii.IndexOf($needle, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
                    throw 'A private machine path remains in the release executable. Packaging stopped.'
                }
            }
        }
    }
}

function Assert-PublicSource {
    param([string]$Path)
    $text = [IO.File]::ReadAllText($Path)
    foreach ($root in @($projectRoot, $env:USERPROFILE, $env:CARGO_HOME, $env:RUSTUP_HOME)) {
        if (-not $root) { continue }
        foreach ($spelling in @($root, $root.Replace('\', '/'))) {
            if ($text.IndexOf($spelling, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
                throw 'A private machine path remains in a source file. Packaging stopped.'
            }
        }
    }
    if ($text -match '(?i)\b[A-Z]:[\\/]+(?:Users|Documents and Settings)[\\/]+|/(?:home|Users)/[A-Za-z0-9._-]+' -or
        $text -match '\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b' -or
        $text -match ('-----BEGIN ' + '(?:(?:RSA|DSA|EC|OPENSSH) )?PRIVATE KEY-----')) {
        throw 'Possible private path or credential material found in source. Packaging stopped.'
    }
}

function Write-FixedZip {
    param([string]$Destination, [object[]]$Entries)
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    if (Test-Path -LiteralPath $Destination) { Remove-Item -LiteralPath $Destination -Force }
    $archive = [IO.Compression.ZipFile]::Open($Destination, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($item in ($Entries | Sort-Object Name)) {
            $entry = $archive.CreateEntry($item.Name.Replace('\', '/'), [IO.Compression.CompressionLevel]::Optimal)
            $entry.LastWriteTime = [DateTimeOffset]::new(2000, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
            $entry.ExternalAttributes = 0
            $inputStream = [IO.File]::OpenRead($item.Path)
            try {
                $outputStream = $entry.Open()
                try { $inputStream.CopyTo($outputStream) }
                finally { $outputStream.Dispose() }
            }
            finally { $inputStream.Dispose() }
        }
    }
    finally { $archive.Dispose() }
    [IO.File]::SetLastWriteTimeUtc($Destination, [DateTime]::SpecifyKind([DateTime]'2000-01-01', [DateTimeKind]::Utc))
}

function Write-SourceArchive {
    param([string]$Version, [string]$DistributionRoot)
    # An allowlist includes hidden GitHub files while excluding Git history,
    # local preferences, editor state, screenshots, caches and executable builds.
    $relativeFiles = [Collections.Generic.List[string]]::new()
    foreach ($name in @('Cargo.toml', 'Cargo.lock', 'build.rs', 'LICENSE', 'README.md', '.gitignore', '.gitattributes')) {
        if (-not (Test-Path -LiteralPath (Join-Path $projectRoot $name) -PathType Leaf)) {
            throw "A required source file is missing: $name"
        }
        $relativeFiles.Add($name)
    }
    $toolbarPreview = 'docs/study-pen-toolbar.png'
    if (Test-Path -LiteralPath (Join-Path $projectRoot $toolbarPreview) -PathType Leaf) {
        $previewItem = Get-Item -LiteralPath (Join-Path $projectRoot $toolbarPreview) -Force
        if (($previewItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'The toolbar preview must be a regular workspace file.'
        }
        $relativeFiles.Add($toolbarPreview)
    }
    $rules = @{
        'src' = @('.rs')
        'docs' = @('.md')
        'scripts' = @('.ps1')
        'resources' = @('.rc', '.ico', '.svg')
        '.github' = @('.yml', '.yaml')
    }
    foreach ($directory in $rules.Keys) {
        $root = Join-Path $projectRoot $directory
        $directoryItem = Get-Item -LiteralPath $root -Force
        if (($directoryItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'Source archive directories must not be symbolic links.'
        }
        foreach ($childDirectory in (Get-ChildItem -LiteralPath $root -Recurse -Directory -Force)) {
            if (($childDirectory.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'Source archive directories must not contain symbolic links.'
            }
        }
        foreach ($file in (Get-ChildItem -LiteralPath $root -Recurse -File -Force | Sort-Object FullName)) {
            if (($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'Source archive inputs must not be symbolic links.'
            }
            if ($rules[$directory] -contains $file.Extension) {
                $relativeFiles.Add($file.FullName.Substring($projectRoot.Length + 1))
            }
        }
    }
    $entries = foreach ($relative in $relativeFiles) {
        $path = Join-Path $projectRoot $relative
        Assert-PublicSource -Path $path
        [PSCustomObject]@{ Path = $path; Name = "study-pen-$Version/$relative" }
    }
    $destination = Join-Path $DistributionRoot "study-pen-$Version-source.zip"
    Write-FixedZip -Destination $destination -Entries @($entries)
    return $destination
}

Push-Location -LiteralPath $projectRoot
$stagingRoot = $null
try {
    $versionMatch = [regex]::Match([IO.File]::ReadAllText((Join-Path $projectRoot 'Cargo.toml')), '(?m)^version = "([0-9]+\.[0-9]+\.[0-9]+)"\s*$')
    if (-not $versionMatch.Success) { throw 'Cargo.toml must contain a stable release version.' }
    $version = $versionMatch.Groups[1].Value
    $distributionRoot = Join-Path $projectRoot 'dist'
    if (Test-Path -LiteralPath $distributionRoot) {
        if (((Get-Item -LiteralPath $distributionRoot -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'The release output directory must be a regular workspace directory.'
        }
    }
    [void][IO.Directory]::CreateDirectory($distributionRoot)
    $portableArchive = Join-Path $distributionRoot 'study-pen-windows-x86_64-portable.zip'
    if (-not $SourceOnly) {
        $hostText = & rustc -vV
        if ($LASTEXITCODE -ne 0) { throw 'Could not determine the Rust target.' }
        $hostTriple = (($hostText | Where-Object { $_ -like 'host: *' }) -replace '^host: ', '').Trim()
        if ($hostTriple -notmatch '^x86_64-pc-windows-(gnu|msvc)$') {
            throw 'Portable releases currently require an x86-64 Windows Rust toolchain.'
        }
        $flags = @(Get-RemappingFlags)
        if ($hostTriple -like '*-msvc') {
            # Bundle the C runtime into the executable. An explicit --target
            # keeps this target-only setting away from host procedural macros.
            $flags += '-Ctarget-feature=+crt-static'
        }
        $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
        if ($GitHubRepository) {
            [Environment]::SetEnvironmentVariable('STUDY_PEN_GITHUB_REPOSITORY', $GitHubRepository, 'Process')
        } else {
            [Environment]::SetEnvironmentVariable('STUDY_PEN_GITHUB_REPOSITORY', [NullString]::Value, 'Process')
        }
        if (-not $SkipTests) {
            Invoke-Cargo -Arguments @('test', '--target', $hostTriple, '--locked')
        }
        Invoke-Cargo -Arguments @('build', '--release', '--target', $hostTriple, '--locked')
        $metadataJson = & cargo metadata --format-version 1 --locked
        if ($LASTEXITCODE -ne 0) { throw 'Could not read dependency metadata.' }
        $metadata = ($metadataJson -join "`n") | ConvertFrom-Json
        $stagingRoot = [IO.Path]::GetFullPath((Join-Path $distributionRoot ('.stage-' + [guid]::NewGuid().ToString('N'))))
        [void][IO.Directory]::CreateDirectory($stagingRoot)
        $executable = Join-Path $stagingRoot 'study-pen.exe'
        Copy-Item -LiteralPath (Join-Path $metadata.target_directory "$hostTriple/release/study-pen.exe") -Destination $executable
        Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/PORTABLE_README.md') -Destination (Join-Path $stagingRoot 'README.md')
        Copy-Item -LiteralPath (Join-Path $projectRoot 'LICENSE') -Destination $stagingRoot
        Write-DependencyNotices -Metadata $metadata -Destination (Join-Path $stagingRoot 'THIRD_PARTY_NOTICES.txt')
        Remove-BuildTimestamp -Executable $executable
        Assert-NoPrivateBuildPaths -Executable $executable
        $checksum = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLowerInvariant()
        [IO.File]::WriteAllText((Join-Path $stagingRoot 'SHA256SUMS.txt'), "$checksum  study-pen.exe`n", [Text.UTF8Encoding]::new($false))
        $entries = foreach ($name in @('study-pen.exe', 'README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.txt', 'SHA256SUMS.txt')) {
            [PSCustomObject]@{ Path = Join-Path $stagingRoot $name; Name = $name }
        }
        Write-FixedZip -Destination $portableArchive -Entries @($entries)
        Write-Output "Portable package: dist/$([IO.Path]::GetFileName($portableArchive))"
    }
    $sourceArchive = Write-SourceArchive -Version $version -DistributionRoot $distributionRoot
    Write-Output "Source package: dist/$([IO.Path]::GetFileName($sourceArchive))"
    $archives = if ($SourceOnly) { @($sourceArchive) } else { @($portableArchive, $sourceArchive) }
    $checksumLines = foreach ($archivePath in $archives) {
        $hash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $([IO.Path]::GetFileName($archivePath))"
    }
    [IO.File]::WriteAllText((Join-Path $distributionRoot "SHA256SUMS-$version.txt"), ($checksumLines -join "`n") + "`n", [Text.UTF8Encoding]::new($false))
    Write-Output "Archive checksums: dist/SHA256SUMS-$version.txt"
}
finally {
    [Environment]::SetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', $originalEncodedFlags, 'Process')
    if ($null -eq $originalUpdateRepository) {
        [Environment]::SetEnvironmentVariable('STUDY_PEN_GITHUB_REPOSITORY', [NullString]::Value, 'Process')
    } else {
        [Environment]::SetEnvironmentVariable('STUDY_PEN_GITHUB_REPOSITORY', $originalUpdateRepository, 'Process')
    }
    if ($stagingRoot -and (Test-Path -LiteralPath $stagingRoot)) {
        # Delete only the exact random staging folder allocated by this run.
        $resolvedStage = [IO.Path]::GetFullPath($stagingRoot)
        $expectedParent = [IO.Path]::GetFullPath((Join-Path $projectRoot 'dist'))
        if ([IO.Path]::GetDirectoryName($resolvedStage) -eq $expectedParent -and
            [IO.Path]::GetFileName($resolvedStage) -match '^\.stage-[0-9a-f]{32}$') {
            Remove-Item -LiteralPath $resolvedStage -Recurse -Force
        }
    }
    Pop-Location
}
