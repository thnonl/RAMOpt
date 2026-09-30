!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec '"$SYSDIR\schtasks.exe" /Delete /TN "RAMOpt" /F'
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "RAMOpt"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "RAMOpt"
!macroend
