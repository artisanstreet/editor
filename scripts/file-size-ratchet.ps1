[CmdletBinding()]
param([string]$Root = (Split-Path -Parent $PSScriptRoot), [int]$Limit = 800, [switch]$Update)
$arguments = @("$PSScriptRoot/file_size_ratchet.py", '--root', $Root, '--limit', $Limit)
if ($Update) { $arguments += '--update' }
python @arguments
exit $LASTEXITCODE
