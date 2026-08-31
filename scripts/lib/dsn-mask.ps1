# DEV-SHELL-DSN-MASK-01: never echo a credential-bearing DSN. Only
# scheme/user/host/port/db are safe to print; password must never appear,
# and an unparsable DSN must not fall back to printing the raw value.
function Get-SafeDsnSummary {
    param([string]$Dsn)

    if ([string]::IsNullOrEmpty($Dsn)) {
        return "(not set)"
    }

    if ($Dsn -match '^(?<scheme>[a-zA-Z][a-zA-Z0-9+.\-]*)://(?:(?<user>[^:@/]+)(?::(?<pass>[^@/]*))?@)?(?<host>[^:/@]+)(?::(?<port>\d+))?(?:/(?<db>[^?]*))?') {
        $scheme = $Matches['scheme']
        $user = $Matches['user']
        $host_ = $Matches['host']
        $port = $Matches['port']
        $db = $Matches['db']

        $userPart = if ($user) { "$user@" } else { "" }
        $portPart = if ($port) { ":$port" } else { "" }
        $dbPart = if ($db) { "/$db" } else { "" }

        return "${scheme}://${userPart}${host_}${portPart}${dbPart}"
    }

    return "(DB config present, cannot be safely summarized)"
}
