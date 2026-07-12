param(
	[switch]$TestHooks
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ($env:ARTISAN_RUN_NATIVE_ADDON_SMOKE -ne "1") {
	throw "Set ARTISAN_RUN_NATIVE_ADDON_SMOKE=1 only in an approved native verification environment"
}

$module_root = Split-Path -Parent $PSScriptRoot
$repository_root = (Resolve-Path (Join-Path $module_root "..\..")).Path
$output_name = if ($TestHooks) {
	"bounded-file-store-native-gnu-test"
} else {
	"bounded-file-store-native-gnu"
}
$output = Join-Path $repository_root ".dist\$output_name"
$target_directory = Join-Path $output "cargo"
$napi_dependency_link_placeholder = Join-Path $output "gnu-node-link-placeholder"
$winget_packages = Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Packages"
$winlibs = Get-ChildItem -LiteralPath $winget_packages -Directory -Filter "BrechtSanders.WinLibs.POSIX.UCRT*" |
	Sort-Object LastWriteTime -Descending |
	Select-Object -First 1

if ($null -eq $winlibs) {
	throw "Install BrechtSanders.WinLibs.POSIX.UCRT with winget before building the GNU addon"
}

$winlibs_bin = Join-Path $winlibs.FullName "mingw64\bin"
$gcc = Join-Path $winlibs_bin "gcc.exe"
$dlltool = Join-Path $winlibs_bin "dlltool.exe"
$napi = Join-Path $module_root "node_modules\.bin\napi.cmd"
$tsc = Join-Path $repository_root "node_modules\.bin\tsc.cmd"
$binding = Join-Path $output "bounded_file_store_native.win32-x64-gnu.node"
$loader_binding = Join-Path $output "bounded_file_store_native.win32-x64-msvc.node"
$loader = Join-Path $output "index.cjs"
$types = Join-Path $output "index.d.ts"
$type_smoke = Join-Path $output "native-type-smoke.ts"

foreach ($required in @($gcc, $dlltool, $napi, $tsc)) {
	if (!(Test-Path -LiteralPath $required -PathType Leaf)) {
		throw "Required native build tool is missing: $required"
	}
}

foreach ($directory in @(
		(Join-Path $repository_root ".dist"),
		$output,
		$target_directory,
		$napi_dependency_link_placeholder
	)) {
	if (
		(Test-Path -LiteralPath $directory) -and
		((Get-Item -Force -LiteralPath $directory).Attributes -band [System.IO.FileAttributes]::ReparsePoint)
	) {
		throw "Refusing to build through a reparse directory: $directory"
	}
}

New-Item -ItemType Directory -Path $napi_dependency_link_placeholder -Force | Out-Null

$placeholder_dll = Join-Path $napi_dependency_link_placeholder "libnode.dll"
$placeholder_library = Join-Path $napi_dependency_link_placeholder "libnode.dll.a"

if (!(Test-Path -LiteralPath $placeholder_dll -PathType Leaf) -or !(Test-Path -LiteralPath $placeholder_library -PathType Leaf)) {
	"__declspec(dllexport) void artisan_node_link_placeholder(void) {}" |
		& $gcc -shared -x c - -o $placeholder_dll "-Wl,--out-implib,$placeholder_library"

	if ($LASTEXITCODE -ne 0) {
		throw "Could not create the GNU Node link placeholder"
	}
}

$environment_names = @(
	"PATH",
	"RUSTUP_TOOLCHAIN",
	"CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER",
	"CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS",
	"LIBNODE_PATH",
	"ARTISAN_RUN_NATIVE_READ_SMOKE"
)
$saved_environment = @{}

foreach ($name in $environment_names) {
	$saved_environment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}

try {
$env:PATH = "$winlibs_bin;$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:RUSTUP_TOOLCHAIN = "1.97.0-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $gcc
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-C dlltool=$dlltool -C link-self-contained=no"
$env:LIBNODE_PATH = $napi_dependency_link_placeholder
$arguments = @(
	"build",
	"--platform",
	"--release",
	"--target",
	"x86_64-pc-windows-gnu",
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

Push-Location $module_root

try {
	& $napi @arguments

	if ($LASTEXITCODE -ne 0) {
		throw "The GNU N-API build failed"
	}
} finally {
	Pop-Location
}

foreach ($generated in @($binding, $loader, $types)) {
	if (!(Test-Path -LiteralPath $generated -PathType Leaf)) {
		throw "The GNU N-API build output was not generated: $generated"
	}
}

$direct_descriptor_json = & node -e "const binding = require(process.argv[1]); process.stdout.write(JSON.stringify(binding.getNativeBuildDescriptor()));" $binding

if ($LASTEXITCODE -ne 0) {
	throw "The direct GNU N-API binding failed to load"
}

$direct_descriptor = $direct_descriptor_json | ConvertFrom-Json

if (
	$direct_descriptor.operatingSystem -ne "windows" -or
	$direct_descriptor.architecture -ne "x86_64" -or
	$direct_descriptor.target -ne "x86_64-pc-windows-gnu" -or
	$direct_descriptor.testHooksEnabled -ne [bool]$TestHooks
) {
	throw "The direct GNU N-API smoke descriptor was unexpected: $($direct_descriptor | ConvertTo-Json -Compress)"
}

if (Test-Path -LiteralPath $loader_binding) {
	throw "Refusing to overwrite an existing generated-loader binding: $loader_binding"
}

$temporary_loader_alias_created = $false

try {
	New-Item -ItemType HardLink -Path $loader_binding -Value $binding -ErrorAction Stop | Out-Null
	$temporary_loader_alias_created = $true

	$loader_descriptor_json = & node -e "const binding = require(process.argv[1]); process.stdout.write(JSON.stringify(binding.getNativeBuildDescriptor()));" $loader

	if ($LASTEXITCODE -ne 0) {
		throw "The generated GNU N-API package loader failed to load"
	}

	$loader_descriptor = $loader_descriptor_json | ConvertFrom-Json

	if (
		$loader_descriptor.operatingSystem -ne "windows" -or
		$loader_descriptor.architecture -ne "x86_64" -or
		$loader_descriptor.target -ne "x86_64-pc-windows-gnu" -or
		$loader_descriptor.testHooksEnabled -ne [bool]$TestHooks
	) {
		throw "The generated-loader GNU smoke descriptor was unexpected: $($loader_descriptor | ConvertTo-Json -Compress)"
	}

	$env:ARTISAN_RUN_NATIVE_READ_SMOKE = "1"
	& node (Join-Path $repository_root ".tests\bounded-file-store-native\native-read-smoke.cjs") $loader

	if ($LASTEXITCODE -ne 0) {
		throw "The native bounded file store read smoke test failed"
	}

	if ($env:ARTISAN_RUN_NATIVE_REPLACE_SMOKE -eq "1") {
		& node (Join-Path $repository_root ".tests\bounded-file-store-native\native-replace-smoke.cjs") $loader

		if ($LASTEXITCODE -ne 0) {
			throw "The native bounded file store replacement smoke test failed"
		}
	}

	if ($env:ARTISAN_RUN_NATIVE_CRASH_SMOKE -eq "1") {
		& node (Join-Path $repository_root ".tests\bounded-file-store-native\native-replace-crash-smoke.cjs") $loader

		if ($LASTEXITCODE -ne 0) {
			throw "The native bounded file store crash recovery smoke test failed"
		}
	}
} finally {
	if ($temporary_loader_alias_created) {
		Remove-Item -LiteralPath $loader_binding -Force
	}
}

if (!$TestHooks) {
	$type_smoke_source = @'
import {
	getNativeBuildDescriptor,
	NativeBoundedRegularFileStore,
	type NativeReplaceRegularFileOptions,
	type NativeBuildDescriptor,
} from "./index";

const descriptor: NativeBuildDescriptor = getNativeBuildDescriptor();
const test_hooks_enabled: boolean = descriptor.testHooksEnabled;
const store = new NativeBoundedRegularFileStore("C:\\", new Uint8Array(32));
const bytes: Promise<Uint8Array> = store.readRegularFile("file.txt", 1024);
const replace_options: NativeReplaceRegularFileOptions = {
	expected: new Uint8Array([1]),
	replacement: new Uint8Array([2]),
	maximumBytes: 1024,
	operationId: "typed-operation",
	path: "file.txt",
};
const replacement: Promise<"Replaced" | "AlreadyReplaced" | "Changed"> =
	store.replaceRegularFile(replace_options);
const finalization: Promise<void> = store.finalizeRegularFileReplacement(replace_options);

void descriptor;
void test_hooks_enabled;
void bytes;
void replacement;
void finalization;
store.close();
'@

	[System.IO.File]::WriteAllText($type_smoke, $type_smoke_source, [System.Text.UTF8Encoding]::new($false))

	try {
		& $tsc --ignoreConfig --noEmit --strict --skipLibCheck --module ESNext --moduleResolution Bundler --target ES2024 $type_smoke

		if ($LASTEXITCODE -ne 0) {
			throw "The generated N-API declaration consumer failed to typecheck"
		}
	} finally {
		Remove-Item -LiteralPath $type_smoke -Force
	}
}

[PSCustomObject]@{
	direct = $direct_descriptor
	loader = $loader_descriptor
} | ConvertTo-Json -Compress
} finally {
	foreach ($name in $environment_names) {
		[Environment]::SetEnvironmentVariable($name, $saved_environment[$name], "Process")
	}
}
