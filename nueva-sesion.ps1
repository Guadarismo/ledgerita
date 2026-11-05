# =====================================================
# Ledgerita - Session report helper
#
# Uso:
#   .\nueva-sesion.ps1            -> crea handoffs\REPORTE_SESION_<fecha>_<hhmm>.md
#                                    desde PLANTILLA_SESION.md, pre-rellenado con
#                                    datos reales de git (branch, commit, estado).
#   .\nueva-sesion.ps1 -Archivar  -> al cierre de sesión: mueve HANDOFF.md a
#                                    handoffs\HANDOFF_<fecha>_<hhmm>.md, agrega
#                                    la linea al INDEX.md y crea un HANDOFF.md
#                                    nuevo (esqueleto a completar).
#
# Requiere: git y PowerShell 5.1+.
# =====================================================
param(
    [switch]$Archivar
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $Root
try {
    function Get-GitInfo {
        $branch = (git rev-parse --abbrev-ref HEAD).Trim()
        $commit = (git log -1 --pretty=format:"%h %s").Trim()
        $dirty = git status --porcelain
        $status = if ($dirty) { "pendiente de commit ($($dirty.Count) cambios)" } else { "working tree limpio" }
        return @{ Branch = $branch; Commit = $commit; Status = $status }
    }

    function Get-Timestamp {
        return "{0}_{1}" -f (Get-Date -Format "yyyy-MM-dd"), (Get-Date -Format "HHmm")
    }

    if ($Archivar) {
        $ts = Get-Timestamp
        $handoff = Join-Path $Root "HANDOFF.md"
        $index   = Join-Path $Root "handoffs\INDEX.md"

        if (Test-Path $handoff) {
            $dest = Join-Path $Root ("handoffs\HANDOFF_" + $ts + ".md")
            Move-Item -LiteralPath $handoff -Destination $dest
            Write-Host ("OK Handoff archivado: " + $dest)
            $resumen = Read-Host "Escribe en UNA linea que hizo tu sesion (para el INDEX.md, Enter para dejar placeholder)"
            if ([string]::IsNullOrWhiteSpace($resumen)) { $resumen = "(completar resumen)" }
            $linea = "| $ts | ``HANDOFF_$ts.md`` | $resumen |"
            Add-Content -LiteralPath $index -Value $linea -Encoding UTF8
            Write-Host "OK Index actualizado"
        } else {
            Write-Warning "No existe HANDOFF.md en la raiz; no hay nada que archivar."
        }

        $git = Get-GitInfo
        $nuevo = @"
# HANDOFF - Ledgerita (en progreso)

> Sesion iniciada: $ts
> Branch: $($git.Branch)
> Ultimo commit: $($git.Commit)
> Estado inicial: $($git.Status)

Estado: sesion en curso. COMPLETAR este archivo al cierre con:
1. Resumen de lo hecho
2. Cambios tecnicos y archivos tocados
3. Produccion verificada (si aplica)
4. Tests resumidos
5. Pendientes de la proxima sesion
6. Trampas nuevas descubiertas (si las hubo)

Convenciones: ver PLANTILLA_SESION.md e historiales en handoffs/INDEX.md
"@
        Set-Content -LiteralPath $handoff -Value $nuevo -Encoding UTF8
        Write-Host "OK HANDOFF.md nuevo creado (esqueleto)"
    }
    else {
        $ts   = Get-Timestamp
        $tpl  = Join-Path $Root "PLANTILLA_SESION.md"
        $dest = Join-Path $Root ("handoffs\REPORTE_SESION_" + $ts + ".md")

        if (-not (Test-Path $tpl)) {
            throw ("No encuentro " + $tpl)
        }
        $git = Get-GitInfo
        $contenido = Get-Content -LiteralPath $tpl -Raw
        $hoy = Get-Date
        $contenido = $contenido.Replace("{{FECHA}}", $hoy.ToString("yyyy-MM-dd"))
        $contenido = $contenido.Replace("{{HORA}}", $hoy.ToString("HH:mm"))
        $contenido = $contenido.Replace("{{BRANCH}}", $git.Branch)
        $contenido = $contenido.Replace("{{COMMIT}}", $git.Commit)
        $contenido = $contenido.Replace("{{ESTADO}}", $git.Status)
        Set-Content -LiteralPath $dest -Value $contenido -Encoding UTF8
        Write-Host ("OK Reporte creado: " + $dest)
        Write-Host "Rellena {{TESTS}} y las demas secciones al cerrar la sesion."
    }
}
finally {
    Pop-Location
}
