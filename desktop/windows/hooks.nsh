; Hooks of the NSIS installer (bundle > windows > nsis > installerHooks in tauri.conf.json).

; "Open with NexSSH" in Explorer's menu for folders (desktop/src/explorer.rs) is there as soon as
; NexSSH is installed. Windows 11's compact menu needs the computer to trust NexSSH's
; certificate: an install the user watches has Windows ask them once (UAC, in front of the
; installer's window). Updates and silent installs never ask; they keep what the computer
; allows (the settings have a button to ask later). The app's own messages go to the details.
!macro NSIS_HOOK_POSTINSTALL
  ${If} $UpdateMode <> 1
  ${AndIf} $PassiveMode <> 1
  ${AndIfNot} ${Silent}
    ${If} $LANGUAGE = 1049 ; Russian
      DetailPrint "Добавление «Открыть через NexSSH» в меню Проводника…"
    ${Else}
      DetailPrint "Adding “Open with NexSSH” to Explorer’s menu…"
    ${EndIf}
    SetDetailsPrint listonly
    nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --explorer-install --trust --window $HWNDPARENT'
    SetDetailsPrint lastused
  ${Else}
    nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --explorer-install'
  ${EndIf}
  Pop $0
!macroend

; Removes what NexSSH added to Explorer's menu for folders, through the app while it is still
; there (desktop/src/explorer.rs): the package and the classic entries, and the computer's trust
; in NexSSH's certificate, for which Windows asks for administrator rights. Updates never
; uninstall. The installer of another version runs this uninstaller first when the user keeps
; "Uninstall before installing", from where it is (Windows' "Uninstall" runs a copy of it from
; the temporary folder instead): then the trust stays, since that installer puts the entries
; back right after, and nobody is asked twice.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ${If} $EXEDIR == $INSTDIR
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --explorer-cleanup --keep-trust'
    ${Else}
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --explorer-cleanup --window $HWNDPARENT'
    ${EndIf}
  ${EndIf}
!macroend

; The classic entries, also when the app could not remove them. Like the template keeps
; shortcuts: only when really uninstalling.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegKey HKCU "Software\Classes\Directory\shell\NexSSH"
    DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\NexSSH"
    DeleteRegKey HKCU "Software\Classes\Drive\shell\NexSSH"
  ${EndIf}
!macroend
