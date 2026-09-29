; Hooks of the NSIS installer (bundle > windows > nsis > installerHooks in tauri.conf.json).

; Removes what NexSSH adds for the user by itself: "Open with NexSSH" in Explorer's menu for
; folders (desktop/src/explorer.rs). Updates never uninstall, but keep it like the template
; keeps shortcuts: only when really uninstalling.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegKey HKCU "Software\Classes\Directory\shell\NexSSH"
    DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\NexSSH"
    DeleteRegKey HKCU "Software\Classes\Drive\shell\NexSSH"
  ${EndIf}
!macroend
