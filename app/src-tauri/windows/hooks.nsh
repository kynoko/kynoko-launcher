; Uninstall = cleanup (docs/SPEC.md, section 11): the launcher removes every
; association, shortcut and registry entry it recorded, before its files go.
; Nothing may point at a program that is no longer there.
;
; The user's CHOICES (browser, profiles, apps, file types) are another matter:
; they go only when "Delete the application data" is ticked on the
; confirmation page (Tauri's own checkbox, unticked by default). Left
; unticked, a reinstall finds them and applies them again.
;
; When a newer installer runs this uninstaller to replace the version in
; place (its reinstall page proposes "Uninstall before installing", checked by
; default), nothing is taken away at all: the new version takes everything
; over, and an update must not turn into a reset. The two cases are told
; apart by where the uninstaller runs. Uninstalled from Windows, NSIS first
; copies it to a temporary folder and runs it from there; called by an
; installer, it runs in place, from the install directory. (StrCmp, under
; ${If}, ignores case.)
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
    ExecWait '"$INSTDIR\kynoko-launcher.exe" cleanup'
  ${ElseIf} $EXEDIR != $INSTDIR
    ExecWait '"$INSTDIR\kynoko-launcher.exe" cleanup --keep-preferences'
  ${EndIf}
!macroend
