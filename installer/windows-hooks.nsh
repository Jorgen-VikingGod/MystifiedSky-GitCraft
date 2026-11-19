!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "Software\Classes\Directory\shell\GitCraft" "" "Open in GitCraft"
  WriteRegStr HKCU "Software\Classes\Directory\shell\GitCraft\command" "" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\"'
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\GitCraft" "" "Open in GitCraft"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\GitCraft\command" "" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%V$\"'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Classes\Directory\shell\GitCraft"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\GitCraft"
!macroend
