; PulseWin URL protocol: navigation-only deep links.
; The URL can select the settings window or an account pane, but it cannot
; change settings, run commands or start a login (enforced by the app).
!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "Software\Classes\pulsewin" "" "URL:PulseWin"
  WriteRegStr HKCU "Software\Classes\pulsewin" "URL Protocol" ""
  WriteRegStr HKCU "Software\Classes\pulsewin\DefaultIcon" "" "$INSTDIR\PulseWin.exe,0"
  WriteRegStr HKCU "Software\Classes\pulsewin\shell" "" ""
  WriteRegStr HKCU "Software\Classes\pulsewin\shell\open" "" ""
  WriteRegStr HKCU "Software\Classes\pulsewin\shell\open\command" "" '"$INSTDIR\PulseWin.exe" "%1"'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Classes\pulsewin"
!macroend
