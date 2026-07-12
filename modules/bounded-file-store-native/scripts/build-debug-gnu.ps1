$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$module_root = Split-Path -Parent $PSScriptRoot
$repository_root = Resolve-Path (Join-Path $module_root "..\..")
$output = Join-Path $repository_root ".dist\bounded-file-store-native"
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

$env:PATH = "$winlibs_bin;$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:RUSTUP_TOOLCHAIN = "1.97.0-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $gcc
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-C dlltool=$dlltool -C link-self-contained=no"
$env:LIBNODE_PATH = $napi_dependency_link_placeholder

Push-Location $module_root

try {
	& $napi build --platform --target x86_64-pc-windows-gnu --target-dir "../../.dist/bounded-file-store-native/cargo" --output-dir "../../.dist/bounded-file-store-native" --js index.cjs

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

if ($direct_descriptor.operatingSystem -ne "windows" -or $direct_descriptor.architecture -ne "x86_64" -or $direct_descriptor.target -ne "x86_64-pc-windows-gnu") {
	throw "The direct GNU N-API smoke descriptor was unexpected: $($direct_descriptor | ConvertTo-Json -Compress)"
}

if (Test-Path -LiteralPath $loader_binding) {
	throw "Refusing to overwrite an existing production MSVC binding: $loader_binding"
}

Copy-Item -LiteralPath $binding -Destination $loader_binding

try {
	$loader_descriptor_json = & node -e "const binding = require(process.argv[1]); process.stdout.write(JSON.stringify(binding.getNativeBuildDescriptor()));" $module_root

	if ($LASTEXITCODE -ne 0) {
		throw "The generated GNU N-API package loader failed to load"
	}

	$loader_descriptor = $loader_descriptor_json | ConvertFrom-Json

	if ($loader_descriptor.operatingSystem -ne "windows" -or $loader_descriptor.architecture -ne "x86_64" -or $loader_descriptor.target -ne "x86_64-pc-windows-gnu") {
		throw "The generated-loader GNU smoke descriptor was unexpected: $($loader_descriptor | ConvertTo-Json -Compress)"
	}

	& node (Join-Path $repository_root ".tests\bounded-file-store-native\native-read-smoke.cjs") $module_root

	if ($LASTEXITCODE -ne 0) {
		throw "The native bounded file store read smoke test failed"
	}

	$replace_smoke_failed = $false

	if ($env:ARTISAN_RUN_NATIVE_REPLACE_SMOKE -eq "1") {
		& node (Join-Path $repository_root ".tests\bounded-file-store-native\native-replace-smoke.cjs") $module_root

		if ($LASTEXITCODE -ne 0) {
			$replace_smoke_failed = $true
		}
	}
} finally {
	Remove-Item -LiteralPath $loader_binding -Force
}

$type_smoke_source = @'
import {
	getNativeBuildDescriptor,
	NativeBoundedRegularFileStore,
	type NativeReplaceRegularFileOptions,
	type NativeBuildDescriptor,
} from "../../modules/bounded-file-store-native";

const descriptor: NativeBuildDescriptor = getNativeBuildDescriptor();
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

if ($replace_smoke_failed) {
	throw "The native bounded file store replacement smoke test failed"
}

[PSCustomObject]@{
	direct = $direct_descriptor
	loader = $loader_descriptor
} | ConvertTo-Json -Compress
