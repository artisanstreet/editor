[CmdletBinding()]
param(
    [string]$Root
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ([string]::IsNullOrWhiteSpace($Root)) {
    $Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
}

$Root = [IO.Path]::GetFullPath($Root)
if (-not (Test-Path -LiteralPath $Root -PathType Container)) {
    throw "Audit root does not exist: $Root"
}

$Findings = [Collections.Generic.List[string]]::new()
$FileTextCache = @{}
$GitMarkerCache = @{}
$PackageFileCache = @{}
$Rules = [Collections.Generic.List[object]]::new()
$RulesByLabel = @{}

function Add-Finding {
    param([string]$Message)

    [void]$script:Findings.Add($Message)
}

function ConvertTo-RepoRelativePath {
    param([string]$Path)

    $fullPath = [IO.Path]::GetFullPath($Path)
    $rootPrefix = $script:Root.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $fullPath.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        return $null
    }

    return $fullPath.Substring($rootPrefix.Length).Replace('\', '/')
}

function Normalize-RepoPath {
    param([string]$Path)

    return $Path.Replace('\', '/').TrimStart('./')
}

function Get-GitRoot {
    try {
        $gitRoot = @(& git -C $script:Root rev-parse --show-toplevel 2>$null)
        if ($LASTEXITCODE -ne 0 -or $gitRoot.Count -eq 0) {
            return $null
        }

        $resolvedGitRoot = [IO.Path]::GetFullPath(([string]$gitRoot[0]).Trim())
        if ($resolvedGitRoot.Equals($script:Root, [StringComparison]::OrdinalIgnoreCase)) {
            return $resolvedGitRoot
        }
    } catch {
        return $null
    }

    return $null
}

$GitRoot = Get-GitRoot
$HasGit = $null -ne $GitRoot

function Get-GitIndexMarker {
    param([string]$RelativePath)

    $relativePath = Normalize-RepoPath $RelativePath
    if (-not $script:HasGit) {
        return $null
    }
    if ($script:GitMarkerCache.ContainsKey($relativePath)) {
        return $script:GitMarkerCache[$relativePath]
    }

    $lines = @(& git -C $script:Root ls-files -t -- $relativePath 2>$null)
    $marker = $null
    if ($LASTEXITCODE -eq 0 -and $lines.Count -gt 0) {
        $line = [string]$lines[0]
        if ($line.Length -ge 2 -and $line.Substring(1).Trim() -eq $relativePath) {
            $marker = $line.Substring(0, 1)
        }
    }

    $script:GitMarkerCache[$relativePath] = $marker
    return $marker
}

function Read-RepositoryText {
    param([string]$RelativePath)

    $relativePath = Normalize-RepoPath $RelativePath
    if ($script:FileTextCache.ContainsKey($relativePath)) {
        return $script:FileTextCache[$relativePath]
    }

    $fullPath = Join-Path $script:Root ($relativePath.Replace('/', [IO.Path]::DirectorySeparatorChar))
    if (Test-Path -LiteralPath $fullPath -PathType Leaf) {
        $text = [IO.File]::ReadAllText($fullPath)
        $script:FileTextCache[$relativePath] = $text
        return $text
    }

    # A worker checkout may deliberately be sparse. A skipped tracked file is
    # still a deterministic input; read its committed bytes without changing
    # the worktree. Ordinary missing/deleted files do not get this fallback.
    if ($script:HasGit -and (Get-GitIndexMarker $relativePath) -eq 'S') {
        $gitText = @(& git -C $script:Root show ("HEAD:" + $relativePath) 2>$null)
        if ($LASTEXITCODE -eq 0) {
            $text = $gitText -join "`n"
            $script:FileTextCache[$relativePath] = $text
            return $text
        }
    }

    throw "Required input file is missing: $relativePath"
}

function Test-RepositoryFile {
    param([string]$RelativePath)

    $relativePath = Normalize-RepoPath $RelativePath
    $fullPath = Join-Path $script:Root ($relativePath.Replace('/', [IO.Path]::DirectorySeparatorChar))
    if (Test-Path -LiteralPath $fullPath -PathType Leaf) {
        return $true
    }

    if ($script:HasGit -and (Get-GitIndexMarker $relativePath) -eq 'S') {
        & git -C $script:Root cat-file -e ("HEAD:" + $relativePath) 2>$null
        return $LASTEXITCODE -eq 0
    }

    return $false
}

function Get-TrackedPaths {
    if (-not $script:HasGit) {
        return @()
    }

    return @(& git -C $script:Root ls-files 2>$null | ForEach-Object {
            Normalize-RepoPath ([string]$_)
        })
}

function Get-InputPathsByName {
    param([string]$FileName)

    $paths = [Collections.Generic.List[string]]::new()
    if (-not $script:HasGit) {
        $physical = Get-ChildItem -LiteralPath $script:Root -Recurse -Force -File -Filter $FileName -ErrorAction SilentlyContinue
        foreach ($file in $physical) {
            $relativePath = ConvertTo-RepoRelativePath $file.FullName
            if ($null -ne $relativePath -and -not $relativePath.StartsWith('.git/', [StringComparison]::OrdinalIgnoreCase)) {
                [void]$paths.Add((Normalize-RepoPath $relativePath))
            }
        }
    }

    foreach ($trackedPath in (Get-TrackedPaths)) {
        if ([IO.Path]::GetFileName($trackedPath) -eq $FileName) {
            [void]$paths.Add($trackedPath)
        }
    }

    return @($paths | Sort-Object -Unique)
}

# This is a deliberately bounded Starlark parser. It recognizes top-level
# named rule calls, keyword assignments, literal strings/lists, and
# glob(["..."], exclude=["..."]) source expressions. It never evaluates
# Starlark or guesses a target label from a filename.
function Get-MatchingParen {
    param(
        [string]$Text,
        [int]$OpenIndex
    )

    $depth = 0
    $quote = $null
    $comment = $false
    for ($i = $OpenIndex; $i -lt $Text.Length; $i++) {
        $char = $Text[$i]
        if ($comment) {
            if ($char -eq "`n") {
                $comment = $false
            }
            continue
        }
        if ($null -ne $quote) {
            if ($char -eq '\' -and $i + 1 -lt $Text.Length) {
                $i++
            } elseif ($char -eq $quote) {
                $quote = $null
            }
            continue
        }
        if ($char -eq '#') {
            $comment = $true
        } elseif ($char -eq '"' -or $char -eq "'") {
            $quote = $char
        } elseif ($char -eq '(') {
            $depth++
        } elseif ($char -eq ')') {
            $depth--
            if ($depth -eq 0) {
                return $i
            }
        }
    }

    return -1
}

function Get-MatchingDelimiter {
    param(
        [string]$Text,
        [int]$OpenIndex,
        [char]$OpenChar,
        [char]$CloseChar
    )

    $depth = 0
    $quote = $null
    $comment = $false
    for ($i = $OpenIndex; $i -lt $Text.Length; $i++) {
        $char = $Text[$i]
        if ($comment) {
            if ($char -eq "`n") {
                $comment = $false
            }
            continue
        }
        if ($null -ne $quote) {
            if ($char -eq '\' -and $i + 1 -lt $Text.Length) {
                $i++
            } elseif ($char -eq $quote) {
                $quote = $null
            }
            continue
        }
        if ($char -eq '#') {
            $comment = $true
        } elseif ($char -eq '"' -or $char -eq "'") {
            $quote = $char
        } elseif ($char -eq $OpenChar) {
            $depth++
        } elseif ($char -eq $CloseChar) {
            $depth--
            if ($depth -eq 0) {
                return $i
            }
        }
    }

    return -1
}

function Get-TopLevelCalls {
    param(
        [string]$Text,
        [string]$RelativePath
    )

    $calls = [Collections.Generic.List[object]]::new()
    $pattern = [regex]'(?m)^[\t ]*([A-Za-z_][A-Za-z0-9_]*)[\t ]*\('
    foreach ($match in $pattern.Matches($Text)) {
        $openIndex = $match.Index + $match.Value.LastIndexOf('(')
        $closeIndex = Get-MatchingParen $Text $openIndex
        if ($closeIndex -lt 0) {
            Add-Finding "${RelativePath}: unmatched parentheses for $($match.Groups[1].Value)"
            continue
        }

        $body = $Text.Substring($openIndex + 1, $closeIndex - $openIndex - 1)
        $line = 1 + (($Text.Substring(0, $match.Index) -split "`n").Count - 1)
        [void]$calls.Add([pscustomobject]@{
                Kind = $match.Groups[1].Value
                Body = $body
                Line = $line
            })
    }

    return @($calls)
}

function Get-TopLevelAssignments {
    param([string]$Body)

    $assignments = [Collections.Generic.List[object]]::new()
    $parenDepth = 0
    $bracketDepth = 0
    $braceDepth = 0
    $quote = $null
    $comment = $false
    $lineStart = $true

    for ($i = 0; $i -lt $Body.Length; $i++) {
        $char = $Body[$i]
        if ($lineStart) {
            $j = $i
            while ($j -lt $Body.Length -and ($Body[$j] -eq ' ' -or $Body[$j] -eq "`t" -or $Body[$j] -eq "`r")) {
                $j++
            }
            if ($j -lt $Body.Length -and $parenDepth -eq 0 -and $bracketDepth -eq 0 -and $braceDepth -eq 0) {
                if ($Body[$j] -match '[A-Za-z_]') {
                    $k = $j + 1
                    while ($k -lt $Body.Length -and $Body[$k] -match '[A-Za-z0-9_]') {
                        $k++
                    }
                    while ($k -lt $Body.Length -and ($Body[$k] -eq ' ' -or $Body[$k] -eq "`t")) {
                        $k++
                    }
                    if ($k -lt $Body.Length -and $Body[$k] -eq '=') {
                        [void]$assignments.Add([pscustomobject]@{
                                Name = (($Body.Substring($j, $k - $j)) -replace '\s*=\s*$', '').Trim()
                                ValueStart = $k + 1
                                Position = $j
                            })
                    }
                }
            }
            $lineStart = $false
        }

        if ($comment) {
            if ($char -eq "`n") {
                $comment = $false
                $lineStart = $true
            }
            continue
        }
        if ($null -ne $quote) {
            if ($char -eq '\' -and $i + 1 -lt $Body.Length) {
                $i++
            } elseif ($char -eq $quote) {
                $quote = $null
            }
            continue
        }
        if ($char -eq '#') {
            $comment = $true
        } elseif ($char -eq '"' -or $char -eq "'") {
            $quote = $char
        } elseif ($char -eq '(') {
            $parenDepth++
        } elseif ($char -eq ')') {
            $parenDepth--
        } elseif ($char -eq '[') {
            $bracketDepth++
        } elseif ($char -eq ']') {
            $bracketDepth--
        } elseif ($char -eq '{') {
            $braceDepth++
        } elseif ($char -eq '}') {
            $braceDepth--
        } elseif ($char -eq "`n") {
            $lineStart = $true
        }
    }

    $values = [Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $assignments.Count; $i++) {
        $start = $assignments[$i].ValueStart
        $end = if ($i + 1 -lt $assignments.Count) { $assignments[$i + 1].Position } else { $Body.Length }
        $value = $Body.Substring($start, $end - $start).Trim()
        [void]$values.Add([pscustomobject]@{
                Name = $assignments[$i].Name
                Value = $value.TrimEnd(',').Trim()
            })
    }

    return @($values)
}

function Get-RuleAttributes {
    param(
        [string]$Body,
        [string]$RuleDescription
    )

    $attributes = @{}
    foreach ($assignment in (Get-TopLevelAssignments $Body)) {
        if ($attributes.ContainsKey($assignment.Name)) {
            Add-Finding "${RuleDescription}: duplicate attribute $($assignment.Name)"
        } else {
            $attributes[$assignment.Name] = $assignment.Value
        }
    }

    return $attributes
}

function Get-StringLiterals {
    param([string]$Expression)

    $literals = [Collections.Generic.List[string]]::new()
    $quote = $null
    $comment = $false
    $buffer = [Text.StringBuilder]::new()
    for ($i = 0; $i -lt $Expression.Length; $i++) {
        $char = $Expression[$i]
        if ($comment) {
            if ($char -eq "`n") {
                $comment = $false
            }
            continue
        }
        if ($null -ne $quote) {
            if ($char -eq '\' -and $i + 1 -lt $Expression.Length) {
                [void]$buffer.Append($Expression[$i + 1])
                $i++
            } elseif ($char -eq $quote) {
                [void]$literals.Add($buffer.ToString())
                $buffer.Clear() | Out-Null
                $quote = $null
            } else {
                [void]$buffer.Append($char)
            }
            continue
        }
        if ($char -eq '#') {
            $comment = $true
        } elseif ($char -eq '"' -or $char -eq "'") {
            $quote = $char
        }
    }

    return @($literals)
}

function Get-IdentifierCallRanges {
    param(
        [string]$Expression,
        [string]$Identifier
    )

    $ranges = [Collections.Generic.List[object]]::new()
    $quote = $null
    $comment = $false
    for ($i = 0; $i -lt $Expression.Length; $i++) {
        $char = $Expression[$i]
        if ($comment) {
            if ($char -eq "`n") {
                $comment = $false
            }
            continue
        }
        if ($null -ne $quote) {
            if ($char -eq '\' -and $i + 1 -lt $Expression.Length) {
                $i++
            } elseif ($char -eq $quote) {
                $quote = $null
            }
            continue
        }
        if ($char -eq '#') {
            $comment = $true
            continue
        }
        if ($char -ne '_' -and $char -notmatch '[A-Za-z]') {
            continue
        }

        $start = $i
        $i++
        while ($i -lt $Expression.Length -and $Expression[$i] -match '[A-Za-z0-9_]') {
            $i++
        }
        $word = $Expression.Substring($start, $i - $start)
        if ($word -ne $Identifier) {
            $i--
            continue
        }
        while ($i -lt $Expression.Length -and ($Expression[$i] -eq ' ' -or $Expression[$i] -eq "`t" -or $Expression[$i] -eq "`r" -or $Expression[$i] -eq "`n")) {
            $i++
        }
        if ($i -lt $Expression.Length -and $Expression[$i] -eq '(') {
            $close = Get-MatchingParen $Expression $i
            if ($close -lt 0) {
                Add-Finding "Unmatched $Identifier call in source expression"
                continue
            }
            [void]$ranges.Add([pscustomobject]@{ Start = $start; End = $close + 1 })
            $i = $close
        } else {
            $i--
        }
    }

    return @($ranges)
}

function Get-GlobMatches {
    param(
        [string]$Package,
        [string]$Pattern,
        [string[]]$Excludes,
        [string]$Description
    )

    $normalizedPattern = Normalize-RepoPath $Pattern
    if ($normalizedPattern.StartsWith('/', [StringComparison]::Ordinal) -or $normalizedPattern.Split('/') -contains '..') {
        Add-Finding "${Description}: unsupported glob path $Pattern"
        return @()
    }

    if (-not $script:PackageFileCache.ContainsKey($Package)) {
        $files = [Collections.Generic.List[string]]::new()
        $packagePath = if ([string]::IsNullOrEmpty($Package)) { $script:Root } else { Join-Path $script:Root ($Package.Replace('/', [IO.Path]::DirectorySeparatorChar)) }
        if (-not $script:HasGit -and (Test-Path -LiteralPath $packagePath -PathType Container)) {
            foreach ($file in (Get-ChildItem -LiteralPath $packagePath -Recurse -Force -File -ErrorAction SilentlyContinue)) {
                $relativePath = ConvertTo-RepoRelativePath $file.FullName
                if ($null -ne $relativePath -and -not $relativePath.StartsWith('.git/', [StringComparison]::OrdinalIgnoreCase)) {
                    [void]$files.Add((Normalize-RepoPath $relativePath))
                }
            }
        }
        $prefix = if ([string]::IsNullOrEmpty($Package)) { '' } else { $Package.TrimEnd('/') + '/' }
        foreach ($trackedPath in (Get-TrackedPaths)) {
            if ($trackedPath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
                [void]$files.Add($trackedPath)
            }
        }
        $script:PackageFileCache[$Package] = @($files | Sort-Object -Unique)
    }

    $regexText = [Text.StringBuilder]::new()
    [void]$regexText.Append('^')
    for ($i = 0; $i -lt $normalizedPattern.Length; $i++) {
        if ($i + 2 -lt $normalizedPattern.Length -and $normalizedPattern.Substring($i, 3) -eq '**/') {
            [void]$regexText.Append('(?:.*/)?')
            $i += 2
        } elseif ($i + 1 -lt $normalizedPattern.Length -and $normalizedPattern.Substring($i, 2) -eq '**') {
            [void]$regexText.Append('.*')
            $i++
        } elseif ($normalizedPattern[$i] -eq '*') {
            [void]$regexText.Append('[^/]*')
        } elseif ($normalizedPattern[$i] -eq '?') {
            [void]$regexText.Append('[^/]')
        } else {
            [void]$regexText.Append([regex]::Escape([string]$normalizedPattern[$i]))
        }
    }
    [void]$regexText.Append('$')
    $patternRegex = [regex]$regexText.ToString()

    $matches = foreach ($file in $script:PackageFileCache[$Package]) {
        $packageRelative = if ([string]::IsNullOrEmpty($Package)) {
            $file
        } else {
            $file.Substring($Package.TrimEnd('/').Length + 1)
        }
        if (-not $patternRegex.IsMatch($packageRelative)) {
            continue
        }
        $excluded = $false
        foreach ($exclude in $Excludes) {
            $excludeRegexText = [Text.StringBuilder]::new()
            [void]$excludeRegexText.Append('^')
            $normalizedExclude = Normalize-RepoPath $exclude
            for ($j = 0; $j -lt $normalizedExclude.Length; $j++) {
                if ($j + 2 -lt $normalizedExclude.Length -and $normalizedExclude.Substring($j, 3) -eq '**/') {
                    [void]$excludeRegexText.Append('(?:.*/)?')
                    $j += 2
                } elseif ($j + 1 -lt $normalizedExclude.Length -and $normalizedExclude.Substring($j, 2) -eq '**') {
                    [void]$excludeRegexText.Append('.*')
                    $j++
                } elseif ($normalizedExclude[$j] -eq '*') {
                    [void]$excludeRegexText.Append('[^/]*')
                } elseif ($normalizedExclude[$j] -eq '?') {
                    [void]$excludeRegexText.Append('[^/]')
                } else {
                    [void]$excludeRegexText.Append([regex]::Escape([string]$normalizedExclude[$j]))
                }
            }
            [void]$excludeRegexText.Append('$')
            if (([regex]$excludeRegexText.ToString()).IsMatch($packageRelative)) {
                $excluded = $true
                break
            }
        }
        if (-not $excluded) {
            $packageRelative
        }
    }

    $matches = @($matches | Sort-Object -Unique)
    if ($matches.Count -eq 0) {
        Add-Finding "${Description}: glob $Pattern matched no repository files"
    }
    return $matches
}

function Resolve-Label {
    param(
        [string]$Label,
        [string]$Package
    )

    if ($Label.StartsWith('//', [StringComparison]::Ordinal)) {
        if ($Label -match '^//[^:]+:.+$') {
            return $Label
        }
        if ($Label -match '^//([^/]+)/?$') {
            $defaultName = ($Matches[1] -split '/')[-1]
            return "//$($Matches[1]):$defaultName"
        }
        if ($Label -eq '//') {
            return '//:BUILD'
        }
        return $Label
    }
    if ($Label.StartsWith(':', [StringComparison]::Ordinal)) {
        return "//${Package}:$($Label.Substring(1))"
    }

    return $null
}

function Resolve-FilePath {
    param(
        [string]$Package,
        [string]$Path
    )

    $packagePath = if ([string]::IsNullOrEmpty($Package)) { $script:Root } else { Join-Path $script:Root ($Package.Replace('/', [IO.Path]::DirectorySeparatorChar)) }
    $fullPath = [IO.Path]::GetFullPath((Join-Path $packagePath ($Path.Replace('/', [IO.Path]::DirectorySeparatorChar))))
    $relativePath = ConvertTo-RepoRelativePath $fullPath
    if ($null -eq $relativePath) {
        return $null
    }

    return Normalize-RepoPath $relativePath
}

function Get-Labels {
    param([string]$Expression)

    return @(Get-StringLiterals $Expression | Where-Object {
            $_.StartsWith('//', [StringComparison]::Ordinal) -or $_.StartsWith(':', [StringComparison]::Ordinal)
        } | Where-Object {
            -not $_.StartsWith('//conditions:', [StringComparison]::Ordinal)
        })
}

function Get-SourceExpressionEntries {
    param(
        [string]$Expression,
        [object]$Rule
    )

    $entries = [Collections.Generic.List[string]]::new()
    $globRanges = @(Get-IdentifierCallRanges $Expression 'glob')
    foreach ($range in $globRanges) {
        $globText = $Expression.Substring($range.Start, $range.End - $range.Start)
        $openIndex = $globText.IndexOf('(')
        $globBody = $globText.Substring($openIndex + 1, $globText.Length - $openIndex - 2)
        $openList = -1
        $quote = $null
        $comment = $false
        for ($i = 0; $i -lt $globBody.Length; $i++) {
            $char = $globBody[$i]
            if ($comment) {
                if ($char -eq "`n") { $comment = $false }
                continue
            }
            if ($null -ne $quote) {
                if ($char -eq '\' -and $i + 1 -lt $globBody.Length) { $i++ }
                elseif ($char -eq $quote) { $quote = $null }
                continue
            }
            if ($char -eq '#') { $comment = $true }
            elseif ($char -eq '"' -or $char -eq "'") { $quote = $char }
            elseif ($char -eq '[') { $openList = $i; break }
        }
        if ($openList -lt 0) {
            Add-Finding "$($Rule.Path):$($Rule.Line): glob must have a literal pattern list"
            continue
        }
        $closeList = Get-MatchingDelimiter $globBody $openList '[' ']'
        if ($closeList -lt 0) {
            Add-Finding "$($Rule.Path):$($Rule.Line): unmatched glob pattern list"
            continue
        }
        $patterns = @(Get-StringLiterals $globBody.Substring($openList + 1, $closeList - $openList - 1))
        $globAttributes = @{}
        foreach ($assignment in (Get-TopLevelAssignments $globBody)) {
            $globAttributes[$assignment.Name] = $assignment.Value
        }
        $excludes = if ($globAttributes.ContainsKey('exclude')) {
            @(Get-StringLiterals $globAttributes['exclude'])
        } else {
            @()
        }
        foreach ($pattern in $patterns) {
            foreach ($match in (Get-GlobMatches $Rule.Package $pattern $excludes "$($Rule.Path):$($Rule.Line)")) {
                [void]$entries.Add($match)
            }
        }
    }

    $remaining = $Expression
    foreach ($range in ($globRanges | Sort-Object Start -Descending)) {
        $remaining = $remaining.Substring(0, $range.Start) + $remaining.Substring($range.End)
    }
    foreach ($literal in (Get-StringLiterals $remaining)) {
        [void]$entries.Add($literal)
    }

    if ($entries.Count -eq 0) {
        Add-Finding "$($Rule.Path):$($Rule.Line): source expression has no supported literal or glob source"
    }
    return @($entries)
}

function Add-SourceReference {
    param(
        [object]$Rule,
        [string]$Value
    )

    if ($Value.StartsWith('@', [StringComparison]::Ordinal)) {
        Add-Finding "$($Rule.Path):$($Rule.Line): external label is not a source file: $Value"
        return
    }

    $resolvedLabel = Resolve-Label $Value $Rule.Package
    if ($null -ne $resolvedLabel -and $script:RulesByLabel.ContainsKey($resolvedLabel)) {
        $Rule.SourceRefs += $resolvedLabel
        return
    }

    $fileValue = $Value
    if ($null -ne $resolvedLabel) {
        if ($Value -match '^//[^:]+:(.+)$') {
            $fileValue = $Matches[1]
            $sourcePackage = $Value.Substring(2, $Value.IndexOf(':', 2) - 2)
        } elseif ($Value.StartsWith(':', [StringComparison]::Ordinal)) {
            $fileValue = $Value.Substring(1)
            $sourcePackage = $Rule.Package
        } else {
            $sourcePackage = $Rule.Package
        }
    } else {
        $sourcePackage = $Rule.Package
    }

    $relativePath = Resolve-FilePath $sourcePackage $fileValue
    if ($null -eq $relativePath) {
        Add-Finding "$($Rule.Path):$($Rule.Line): source path escapes repository: $Value"
        return
    }
    if (-not (Test-RepositoryFile $relativePath)) {
        Add-Finding "$($Rule.Path):$($Rule.Line): missing source file $relativePath"
        return
    }
    if ($Rule.SourceFileSet.ContainsKey($relativePath)) {
        Add-Finding "$($Rule.Path):$($Rule.Line): source is listed more than once in $($Rule.Label): $relativePath"
        return
    }

    $Rule.SourceFileSet[$relativePath] = $true
    $Rule.SourceFiles += $relativePath
}

function Assert-SourceReference {
    param(
        [object]$Rule,
        [string]$Value,
        [string]$Attribute
    )

    $resolvedLabel = Resolve-Label $Value $Rule.Package
    if ($null -ne $resolvedLabel -and $script:RulesByLabel.ContainsKey($resolvedLabel)) {
        return
    }
    if ($Value.StartsWith('@', [StringComparison]::Ordinal)) {
        Add-Finding "$($Rule.Path):$($Rule.Line): external label is not a source file in ${Attribute}: $Value"
        return
    }

    $sourcePackage = $Rule.Package
    $fileValue = $Value
    if ($null -ne $resolvedLabel -and $Value -match '^//[^:]+:(.+)$') {
        $sourcePackage = $Value.Substring(2, $Value.IndexOf(':', 2) - 2)
        $fileValue = $Matches[1]
    } elseif ($null -ne $resolvedLabel -and $Value.StartsWith(':', [StringComparison]::Ordinal)) {
        $fileValue = $Value.Substring(1)
    }
    $relativePath = Resolve-FilePath $sourcePackage $fileValue
    if ($null -eq $relativePath -or -not (Test-RepositoryFile $relativePath)) {
        Add-Finding "$($Rule.Path):$($Rule.Line): missing source file $relativePath for $Attribute"
    }
}

function Add-RuleDependencies {
    param([object]$Rule)

    foreach ($attributeName in @('deps', 'crate', 'proc_macro_deps')) {
        if (-not $Rule.Attributes.ContainsKey($attributeName)) {
            continue
        }
        foreach ($labelText in (Get-Labels $Rule.Attributes[$attributeName])) {
            $resolvedLabel = Resolve-Label $labelText $Rule.Package
            if ($null -eq $resolvedLabel -or -not $script:RulesByLabel.ContainsKey($resolvedLabel)) {
                Add-Finding "$($Rule.Path):$($Rule.Line): unknown Rust dependency label $labelText"
                continue
            }
            $dependency = $script:RulesByLabel[$resolvedLabel]
            if ($dependency.Kind -in @('rust_library', 'rust_binary', 'rust_test')) {
                $Rule.Deps += $resolvedLabel
            }
        }
    }
}

function Get-AggregateMembers {
    param(
        [object]$Aggregate,
        [string]$AttributeName
    )

    if (-not $Aggregate.Attributes.ContainsKey($AttributeName)) {
        Add-Finding "$($Aggregate.Path):$($Aggregate.Line): $($Aggregate.Kind) is missing $AttributeName"
        return @()
    }

    return @(Get-Labels $Aggregate.Attributes[$AttributeName])
}

function Resolve-RustMembers {
    param(
        [object]$Aggregate,
        [string[]]$Labels
    )

    $resolved = [Collections.Generic.List[object]]::new()
    foreach ($labelText in $Labels) {
        $label = Resolve-Label $labelText $Aggregate.Package
        if ($null -eq $label -or -not $script:RulesByLabel.ContainsKey($label)) {
            Add-Finding "$($Aggregate.Path):$($Aggregate.Line): aggregate references unknown target $labelText"
            continue
        }
        $rule = $script:RulesByLabel[$label]
        if ($rule.Kind -notin @('rust_library', 'rust_binary', 'rust_test')) {
            Add-Finding "$($Aggregate.Path):$($Aggregate.Line): aggregate references non-Rust target $labelText"
            continue
        }
        [void]$resolved.Add($rule)
    }

    return @($resolved)
}

function Get-RustClosure {
    param([object[]]$Roots)

    $seen = @{}
    $queue = [Collections.Generic.Queue[object]]::new()
    foreach ($root in $Roots) {
        if (-not $seen.ContainsKey($root.Label)) {
            $seen[$root.Label] = $root
            $queue.Enqueue($root)
        }
    }
    while ($queue.Count -gt 0) {
        $current = $queue.Dequeue()
        foreach ($dependencyLabel in $current.Deps) {
            if (-not $seen.ContainsKey($dependencyLabel)) {
                $dependency = $script:RulesByLabel[$dependencyLabel]
                $seen[$dependencyLabel] = $dependency
                $queue.Enqueue($dependency)
            }
        }
    }

    return @($seen.Values)
}

function Check-QualityAggregate {
    param(
        [object]$Aggregate,
        [string]$AttributeName,
        [bool]$IsRoot
    )

    $labelTexts = @(Get-AggregateMembers $Aggregate $AttributeName)
    foreach ($duplicate in ($labelTexts | Group-Object | Where-Object Count -gt 1)) {
        Add-Finding "$($Aggregate.Path):$($Aggregate.Line): duplicate aggregate label $($duplicate.Name)"
    }
    $members = @(Resolve-RustMembers $Aggregate $labelTexts)
    $memberSet = @{}
    foreach ($member in $members) {
        $memberSet[$member.Label] = $true
    }

    $required = if ($IsRoot) {
        @($script:Rules | Where-Object Kind -in @('rust_library', 'rust_binary', 'rust_test'))
    } else {
        @($script:Rules | Where-Object {
                $_.Package -eq $Aggregate.Package -and $_.Kind -in @('rust_library', 'rust_binary', 'rust_test')
            })
    }

    if ($Aggregate.Kind -eq 'rust_clippy') {
        foreach ($target in $required) {
            if (-not $memberSet.ContainsKey($target.Label)) {
                Add-Finding "$($Aggregate.Path):$($Aggregate.Line): $($target.Label) is missing from direct clippy membership"
            }
        }
        return
    }

    if (-not $Aggregate.Attributes.ContainsKey('transitive') -or $Aggregate.Attributes['transitive'] -notmatch '^True$') {
        Add-Finding "$($Aggregate.Path):$($Aggregate.Line): rustfmt_test must declare transitive = True"
    }
    $covered = @(Get-RustClosure $members)
    $coveredSet = @{}
    foreach ($target in $covered) {
        $coveredSet[$target.Label] = $true
    }
    foreach ($target in $required) {
        if (-not $coveredSet.ContainsKey($target.Label)) {
            Add-Finding "$($Aggregate.Path):$($Aggregate.Line): $($target.Label) is missing from rustfmt coverage"
        }
    }
}

function Get-CargoTargets {
    param(
        [string]$ManifestPath,
        [string]$Text
    )

    $targets = [Collections.Generic.List[object]]::new()
    $kind = $null
    $name = $null
    $path = $null
    $sectionLine = 0
    $finish = {
        if ($null -eq $kind) {
            return
        }
        if ([string]::IsNullOrWhiteSpace($path)) {
            Add-Finding "${ManifestPath}:${sectionLine}: [[$kind]] has no explicit path"
        } else {
            [void]$targets.Add([pscustomobject]@{
                    Manifest = $ManifestPath
                    Kind = $kind
                    Name = $name
                    Path = $path
                    Line = $sectionLine
                })
        }
    }

    $lines = $Text -split "`r?`n"
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $trimmed = $lines[$i].Trim()
        if ($trimmed -eq '[[bin]]' -or $trimmed -eq '[[test]]') {
            & $finish
            $kind = if ($trimmed -eq '[[bin]]') { 'bin' } else { 'test' }
            $name = $null
            $path = $null
            $sectionLine = $i + 1
        } elseif ($trimmed -match '^\[\[' -or $trimmed -match '^\[') {
            & $finish
            $kind = $null
            $name = $null
            $path = $null
        } elseif ($null -ne $kind) {
            if ($lines[$i] -match '^\s*name\s*=\s*"([^"]+)"') {
                $name = $Matches[1]
            } elseif ($lines[$i] -match '^\s*path\s*=\s*"([^"]+)"') {
                if ($null -ne $path) {
                    Add-Finding "${ManifestPath}:${sectionLine}: [[$kind]] has duplicate path attributes"
                }
                $path = $Matches[1]
            }
        }
    }
    & $finish

    return @($targets)
}

# Discover actual BUILD files from the worktree. The Git path fallback covers
# only skip-worktree files; a normal missing file remains an audit finding.
$buildPaths = Get-InputPathsByName 'BUILD.bazel'
$buildPaths += Get-InputPathsByName 'BUILD'
$buildPaths = @($buildPaths | Sort-Object -Unique)
if ($buildPaths.Count -eq 0) {
    throw 'No BUILD or BUILD.bazel files were found'
}

foreach ($buildPath in $buildPaths) {
    $normalizedBuildPath = Normalize-RepoPath $buildPath
    $package = if ($normalizedBuildPath -match '^(.*)/BUILD(?:\.bazel)?$') { $Matches[1] } else { '' }
    $text = Read-RepositoryText $normalizedBuildPath
    foreach ($call in (Get-TopLevelCalls $text $normalizedBuildPath)) {
        $attributes = Get-RuleAttributes $call.Body "${normalizedBuildPath}:$($call.Line)"
        if (-not $attributes.ContainsKey('name')) {
            continue
        }
        $nameValue = @(Get-StringLiterals $attributes['name'])
        if ($nameValue.Count -ne 1) {
            Add-Finding "${normalizedBuildPath}:$($call.Line): name must be one literal string"
            continue
        }
        $label = "//${package}:$($nameValue[0])"
        $rule = [pscustomobject]@{
            Kind = $call.Kind
            Name = $nameValue[0]
            Label = $label
            Package = $package
            Path = $normalizedBuildPath
            Line = $call.Line
            Attributes = $attributes
            SourceFiles = @()
            SourceFileSet = @{}
            SourceRefs = @()
            Deps = @()
            IsTestTarget = ($call.Kind -eq 'rust_test' -or ($attributes.ContainsKey('testonly') -and $attributes['testonly'] -match '^True$'))
        }
        if ($script:RulesByLabel.ContainsKey($label)) {
            Add-Finding "${normalizedBuildPath}:$($call.Line): duplicate target label $label"
        } else {
            $script:RulesByLabel[$label] = $rule
        }
        [void]$script:Rules.Add($rule)
    }
}

$rustRules = @($Rules | Where-Object Kind -in @('rust_library', 'rust_binary', 'rust_test'))
$sourceRules = @($Rules | Where-Object Kind -in @('rust_library', 'rust_binary', 'rust_test', 'capnp_codegen'))
foreach ($rule in $sourceRules) {
    $hasSrcs = $rule.Attributes.ContainsKey('srcs')
    $hasCrate = $rule.Attributes.ContainsKey('crate')
    if ($rule.Kind -in @('rust_library', 'rust_binary') -and -not $hasSrcs) {
        Add-Finding "$($rule.Path):$($rule.Line): $($rule.Kind) $($rule.Label) has no srcs"
    }
    if ($rule.Kind -eq 'rust_test' -and -not $hasSrcs -and -not $hasCrate) {
        Add-Finding "$($rule.Path):$($rule.Line): rust_test $($rule.Label) has neither srcs nor crate"
    }
    if ($rule.Kind -eq 'capnp_codegen' -and -not $hasSrcs) {
        Add-Finding "$($rule.Path):$($rule.Line): capnp_codegen $($rule.Label) has no srcs"
    }
    if ($hasSrcs) {
        foreach ($sourceValue in (Get-SourceExpressionEntries $rule.Attributes['srcs'] $rule)) {
            Add-SourceReference $rule $sourceValue
        }
    }
    if ($rule.Kind -in @('rust_library', 'rust_binary', 'rust_test') -and $rule.Attributes.ContainsKey('crate_root')) {
        foreach ($crateRoot in (Get-StringLiterals $rule.Attributes['crate_root'])) {
            Assert-SourceReference $rule $crateRoot 'crate_root'
        }
    }
    Add-RuleDependencies $rule
}

$sourceOwners = @{}
foreach ($rule in $rustRules) {
    foreach ($sourceFile in $rule.SourceFiles) {
        if (-not $sourceOwners.ContainsKey($sourceFile)) {
            $sourceOwners[$sourceFile] = [Collections.Generic.List[object]]::new()
        }
        [void]$sourceOwners[$sourceFile].Add($rule)
    }
}
foreach ($sourceOwner in $sourceOwners.GetEnumerator()) {
    $owners = @($sourceOwner.Value)
    $productionOwners = @($owners | Where-Object { -not $_.IsTestTarget })
    if ($productionOwners.Count -gt 1) {
        $labels = ($productionOwners | ForEach-Object Label) -join ', '
        Add-Finding "Rust source has multiple production owners: $($sourceOwner.Key) [$labels]"
    }
}

$qualityAggregates = @($Rules | Where-Object Kind -in @('rust_clippy', 'rustfmt_test'))
$rootClippy = @($qualityAggregates | Where-Object { $_.Package -eq '' -and $_.Kind -eq 'rust_clippy' -and $_.Name -eq 'clippy' })
$rootFormat = @($qualityAggregates | Where-Object { $_.Package -eq '' -and $_.Kind -eq 'rustfmt_test' -and $_.Name -eq 'format_test' })
if ($rootClippy.Count -ne 1) {
    Add-Finding "Expected exactly one root //:clippy aggregate, found $($rootClippy.Count)"
} else {
    Check-QualityAggregate $rootClippy[0] 'deps' $true
}
if ($rootFormat.Count -ne 1) {
    Add-Finding "Expected exactly one root //:format_test aggregate, found $($rootFormat.Count)"
} else {
    Check-QualityAggregate $rootFormat[0] 'targets' $true
}

foreach ($aggregate in ($qualityAggregates | Where-Object { $_.Package -ne '' })) {
    $attributeName = if ($aggregate.Kind -eq 'rust_clippy') { 'deps' } else { 'targets' }
    Check-QualityAggregate $aggregate $attributeName $false
}

$cargoTargets = [Collections.Generic.List[object]]::new()
foreach ($manifestPath in (Get-InputPathsByName 'Cargo.toml')) {
    foreach ($cargoTarget in (Get-CargoTargets $manifestPath (Read-RepositoryText $manifestPath))) {
        $manifestDirectory = Split-Path -Parent $manifestPath
        $relativePath = Resolve-FilePath $manifestDirectory $cargoTarget.Path
        if ($null -eq $relativePath -or -not (Test-RepositoryFile $relativePath)) {
            Add-Finding "${manifestPath}:$($cargoTarget.Line): missing Cargo [[$($cargoTarget.Kind)]] path $($cargoTarget.Path)"
        }
        $cargoTarget | Add-Member -NotePropertyName ResolvedPath -NotePropertyValue $relativePath
        [void]$cargoTargets.Add($cargoTarget)
    }
}
$cargoPathGroups = @($cargoTargets | Where-Object { $null -ne $_.ResolvedPath } | Group-Object { $_.ResolvedPath.ToLowerInvariant() })
foreach ($group in ($cargoPathGroups | Where-Object Count -gt 1)) {
    $descriptions = ($group.Group | ForEach-Object { "$($_.Manifest) [[$($_.Kind)]] $($_.Name)" }) -join ', '
    Add-Finding "Cargo target path is duplicated: $($group.Name) [$descriptions]"
}

$rustLibraries = @($rustRules | Where-Object Kind -eq 'rust_library')
$rustBinaries = @($rustRules | Where-Object Kind -eq 'rust_binary')
$rustTests = @($rustRules | Where-Object Kind -eq 'rust_test')
$rootClippyCount = if ($rootClippy.Count -eq 1) { @(Get-AggregateMembers $rootClippy[0] 'deps').Count } else { 0 }
$rootFormatCount = if ($rootFormat.Count -eq 1) { @(Get-AggregateMembers $rootFormat[0] 'targets').Count } else { 0 }
Write-Output ("Rust target registration audit: {0} libraries, {1} binaries, {2} tests; {3} Cargo bin/test paths." -f $rustLibraries.Count, $rustBinaries.Count, $rustTests.Count, $cargoTargets.Count)
Write-Output ("Root //:clippy direct members: {0}; root //:format_test direct roots: {1}." -f $rootClippyCount, $rootFormatCount)
Write-Output ("Checked {0} package-level rust_clippy/rustfmt_test aggregates, source ownership, BUILD source paths, and Cargo target paths." -f @($qualityAggregates | Where-Object Package -ne '').Count)

if ($Findings.Count -gt 0) {
    [Console]::Error.WriteLine("Rust target registration audit failed with $($Findings.Count) finding(s):")
    foreach ($finding in $Findings) {
        [Console]::Error.WriteLine("- $finding")
    }
    exit 1
}

Write-Output 'Rust target registration audit passed with zero findings.'
exit 0
