param(
	[Parameter(Mandatory = $true)]
	[ValidateSet("Add", "Remove")]
	[string] $Action,
	[Parameter(Mandatory = $true)]
	[string] $BinPath
)

$ErrorActionPreference = "Stop"

$target = [System.IO.Path]::GetFullPath($BinPath).TrimEnd(
	[System.IO.Path]::DirectorySeparatorChar,
	[System.IO.Path]::AltDirectorySeparatorChar
)
$current = [Environment]::GetEnvironmentVariable(
	"Path",
	[EnvironmentVariableTarget]::User
)
$entries = if ([string]::IsNullOrWhiteSpace($current)) {
	@()
} else {
	@($current.Split(";") | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
}
$updated = [System.Collections.Generic.List[string]]::new()
foreach ($entry in $entries) {
	$normalized = $entry.Trim().TrimEnd(
		[System.IO.Path]::DirectorySeparatorChar,
		[System.IO.Path]::AltDirectorySeparatorChar
	)
	if (-not [string]::Equals(
		$normalized,
		$target,
		[StringComparison]::OrdinalIgnoreCase
	)) {
		$updated.Add($entry.Trim())
	}
}
if ($Action -eq "Add") {
	$updated.Add($target)
}

[Environment]::SetEnvironmentVariable(
	"Path",
	[string]::Join(";", $updated),
	[EnvironmentVariableTarget]::User
)
