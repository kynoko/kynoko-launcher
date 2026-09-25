; Uninstall = cleanup (docs/SPEC.md, section 11): the launcher removes every
; association, shortcut and registry entry it recorded, before its files go.
!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\kynoko-launcher.exe" cleanup'
!macroend
