#Requires -Version 5.1
<#
.SYNOPSIS
    Syncs the stratagem catalog from helldivers.wiki.gg.

.DESCRIPTION
    Queries the wiki Cargo table "Stratagems" for every stratagem name, permit,
    type, and input code, downloads each icon file, and writes:
        data/stratagems.json      catalog consumed by the app
        data/stratagems/<id>.svg  reference icons (original wiki files)

    Re-run whenever a warbond adds stratagems, then review the diff and commit.
    The app itself never touches the network; it embeds the committed data.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools/sync-stratagems.ps1
#>
[CmdletBinding()]
param(
    [string]$Root = ""
)

$ErrorActionPreference = 'Stop'
if (-not $Root) { $Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path) }
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Api = 'https://helldivers.wiki.gg/api.php'
$SourcePage = 'https://helldivers.wiki.gg/wiki/Stratagems'
$Headers = @{ 'User-Agent' = 'XilousStratagemsManager-sync/1.0 (+https://github.com/Xilous/xilous-stratagems-manager)' }
$JsonPath = Join-Path $Root 'data\stratagems.json'
$IconDir = Join-Path $Root 'data\stratagems'

function Invoke-WikiApi([hashtable]$Params) {
    $query = ($Params.GetEnumerator() | ForEach-Object {
        '{0}={1}' -f $_.Key, [Uri]::EscapeDataString([string]$_.Value)
    }) -join '&'
    Invoke-RestMethod -Uri "$Api`?$query" -Headers $Headers -Method Get
}

function ConvertTo-Slug([string]$Name) {
    $slug = $Name.ToLowerInvariant() -replace '[^a-z0-9]+', '-'
    $slug.Trim('-')
}

function ConvertFrom-CodeWikitext([string]$Wikitext) {
    $arrows = [regex]::Matches($Wikitext, 'Stratagem Arrow (Up|Down|Left|Right)\.svg')
    @($arrows | ForEach-Object { $_.Groups[1].Value.ToLowerInvariant() })
}

# --- 1. Cargo rows ---------------------------------------------------------
Write-Host 'Querying Cargo table "Stratagems"...'
$rows = @()
$offset = 0
do {
    $page = Invoke-WikiApi @{
        action = 'cargoquery'; tables = 'Stratagems'
        fields = 'title,image,permit_type,stratagem_type,stratagem_code,base_cooldown'
        limit = '500'; offset = "$offset"; format = 'json'
    }
    $batch = @($page.cargoquery | ForEach-Object { $_.title })
    $rows += $batch
    $offset += $batch.Count
} while ($batch.Count -eq 500)
Write-Host "  $($rows.Count) rows"

# --- 2. Build entries --------------------------------------------------------
$entries = @{}
foreach ($row in $rows) {
    $name = [string]$row.title
    if ([string]::IsNullOrWhiteSpace($name)) { continue }
    $id = ConvertTo-Slug $name
    if ($entries.ContainsKey($id)) {
        Write-Warning "duplicate id '$id' for '$name'; keeping first"
        continue
    }
    $entries[$id] = [ordered]@{
        id = $id
        name = $name
        permit = [string]$row.'permit type'
        type = [string]$row.'stratagem type'
        code = ConvertFrom-CodeWikitext ([string]$row.'stratagem code')
        cooldown = [string]$row.'base cooldown'
        wiki_image = [string]$row.image
        icon = $null
    }
}

# --- 3. Icon files -------------------------------------------------------------
New-Item -ItemType Directory -Force -Path $IconDir | Out-Null
$withImage = @($entries.Values | Where-Object { -not [string]::IsNullOrWhiteSpace($_.wiki_image) })
Write-Host "Resolving $($withImage.Count) icon files..."
$fileUrls = @{}
for ($i = 0; $i -lt $withImage.Count; $i += 50) {
    $chunk = $withImage[$i..([Math]::Min($i + 49, $withImage.Count - 1))]
    $titles = ($chunk | ForEach-Object { 'File:' + $_.wiki_image }) -join '|'
    $result = Invoke-WikiApi @{
        action = 'query'; titles = $titles; prop = 'imageinfo'; iiprop = 'url'; format = 'json'
    }
    # Map any title normalization back to what we asked for.
    $normalized = @{}
    foreach ($n in @($result.query.normalized)) { if ($n) { $normalized[$n.to] = $n.from } }
    foreach ($prop in $result.query.pages.PSObject.Properties) {
        $p = $prop.Value
        $title = if ($normalized.ContainsKey($p.title)) { $normalized[$p.title] } else { $p.title }
        $url = $p.imageinfo | Select-Object -First 1 -ExpandProperty url -ErrorAction SilentlyContinue
        if ($url) { $fileUrls[$title] = $url }
    }
}

$downloaded = 0
foreach ($entry in $withImage) {
    $key = 'File:' + $entry.wiki_image
    if (-not $fileUrls.ContainsKey($key)) {
        Write-Warning "no file url for '$($entry.name)' ($($entry.wiki_image))"
        continue
    }
    $ext = [IO.Path]::GetExtension($entry.wiki_image).ToLowerInvariant()
    if ($ext -ne '.svg' -and $ext -ne '.png') {
        Write-Warning "unsupported icon format for '$($entry.name)': $ext"
        continue
    }
    $file = "$($entry.id)$ext"
    $target = Join-Path $IconDir $file
    try {
        Invoke-WebRequest -Uri $fileUrls[$key] -Headers $Headers -OutFile $target -UseBasicParsing
        $entry.icon = $file
        $downloaded++
    } catch {
        Write-Warning "download failed for '$($entry.name)': $($_.Exception.Message)"
    }
}
Write-Host "  $downloaded icons written to $IconDir"

# --- 4. Remove icons that no longer belong to a catalog entry -----------------
$keep = @($entries.Values | Where-Object { $_.icon } | ForEach-Object { $_.icon })
Get-ChildItem $IconDir -File | Where-Object { $keep -notcontains $_.Name } | ForEach-Object {
    Write-Host "  removing stale icon $($_.Name)"
    Remove-Item $_.FullName -Force
}

# --- 5. Write catalog ----------------------------------------------------------
$catalog = [ordered]@{
    source = $SourcePage
    synced_at = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    stratagems = @($entries.Values | Sort-Object { $_.name })
}
$json = $catalog | ConvertTo-Json -Depth 6
[IO.File]::WriteAllText($JsonPath, $json + "`n", [Text.UTF8Encoding]::new($false))
$withCode = @($entries.Values | Where-Object { $_.code.Count -gt 0 }).Count
Write-Host "Wrote $JsonPath ($($entries.Count) stratagems, $withCode with input codes)"
