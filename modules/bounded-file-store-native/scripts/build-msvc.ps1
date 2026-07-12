param(
	[switch]$TestHooks
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$module_root = Split-Path -Parent $PSScriptRoot
$repository_root = (Resolve-Path (Join-Path $module_root "..\..")).Path
$output_name = if ($TestHooks) {
	"bounded-file-store-native-test"
} else {
	"bounded-file-store-native"
}
$output = Join-Path $repository_root ".dist\$output_name"
$target_directory = Join-Path $output "cargo"
$napi = Join-Path $module_root "node_modules\.bin\napi.cmd"
$generated_files = @(
	(Join-Path $output "bounded_file_store_native.win32-x64-msvc.node"),
	(Join-Path $output "index.cjs"),
	(Join-Path $output "index.d.ts")
)
$arguments = @(
	"build",
	"--platform",
	"--release",
	"--target",
	"x86_64-pc-windows-msvc",
	"--target-dir",
	$target_directory,
	"--output-dir",
	$output,
	"--js",
	"index.cjs"
)

if ($TestHooks) {
	$arguments += @("--features", "native-test-hooks")
}

if (!(Test-Path -LiteralPath $napi -PathType Leaf)) {
	throw "Required NAPI build tool is missing: $napi"
}

foreach ($directory in @((Join-Path $repository_root ".dist"), $output)) {
	if (
		(Test-Path -LiteralPath $directory) -and
		((Get-Item -Force -LiteralPath $directory).Attributes -band [System.IO.FileAttributes]::ReparsePoint)
	) {
		throw "Refusing to build through a reparse directory: $directory"
	}
}

foreach ($generated in $generated_files) {
	if (Test-Path -LiteralPath $generated) {
		Remove-Item -Force -LiteralPath $generated
	}
}

Push-Location $module_root

try {
	& $napi @arguments

	if ($LASTEXITCODE -ne 0) {
		throw "The MSVC N-API build failed"
	}
} finally {
	Pop-Location
}

foreach ($generated in $generated_files) {
	if (!(Test-Path -LiteralPath $generated -PathType Leaf)) {
		throw "The MSVC N-API build output was not generated: $generated"
	}
}
