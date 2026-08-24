# =============================================================================
# CanonicalPaperDbCanary.ps1
#
# D-R2-R4: shared classification authority for the ONE narrow secret-canary
# exemption that BOTH the pre-stage scan (Backup-MiniQuantDeskRecovery.ps1)
# and the post-restore re-scan (Invoke-MiniQuantDeskOffsiteBackup.ps1) must
# apply identically -- the exact, already-public canonical local Paper
# Postgres loopback dev default (identical in both Start-MiniQuantDesk.ps1
# and Start-PaperTradingSmoke.ps1). Both scripts dot-source this file so the
# two scanners can never semantically drift from each other again.
#
# This does NOT exempt MQK_DATABASE_URL in general, all loopback URLs, or
# all Postgres URLs -- only this one exact, fully structurally-verified
# value. Every other allowlisted secret value remains fail-closed.
# =============================================================================

$CanonicalLocalPaperDatabaseUrl = 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable'

function Test-IsCanonicalLocalPaperDatabaseUrl {
    param([string]$Value)
    # Exact-string equality against the known-public canonical value is
    # already sufficient on its own, but a full structural re-check is kept
    # as an explicit, independent guard: a future edit that weakens the
    # string comparison (e.g. to a prefix/contains check) would still be
    # caught here before the exemption could silently broaden.
    if ($Value -cne $CanonicalLocalPaperDatabaseUrl) { return $false }
    $uri = $null
    try { $uri = [System.Uri]$Value } catch { return $false }
    if ($uri.Scheme -notin @('postgres', 'postgresql')) { return $false }
    if ($uri.Host -notin @('127.0.0.1', 'localhost', '::1')) { return $false }
    if ($uri.Port -ne 5440) { return $false }
    if ($uri.AbsolutePath.TrimStart('/') -ne 'miniquantdesk_paper') { return $false }
    $secretLikeParamNames = @('token', 'secret', 'key', 'apikey', 'api_key', 'auth', 'password', 'passwd', 'credential')
    foreach ($pair in $uri.Query.TrimStart('?').Split('&')) {
        if ([string]::IsNullOrEmpty($pair)) { continue }
        $paramName = $pair.Split('=')[0]
        foreach ($pat in $secretLikeParamNames) {
            if ($paramName -match "(?i)$pat") { return $false }
        }
    }
    return $true
}
