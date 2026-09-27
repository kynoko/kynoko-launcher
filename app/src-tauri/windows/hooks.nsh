; Uninstall = cleanup (docs/SPEC.md, section 11): the launcher removes every
; association, shortcut and registry entry it recorded, before its files go.
;
; EXCEPT when a newer installer runs this uninstaller to replace the version
; in place (its reinstall page proposes "Uninstall before installing", checked
; by default): the new version takes the user's choices over, and wiping them
; would turn an update into a reset. The two cases are told apart by where the
; uninstaller runs. Uninstalled from Windows, NSIS first copies it to a
; temporary folder and runs it from there; called by an installer, it runs in
; place, from the install directory. (StrCmp, under ${If}, ignores case.)
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $EXEDIR != $INSTDIR
    ExecWait '"$INSTDIR\kynoko-launcher.exe" cleanup'
  ${EndIf}
!macroend
