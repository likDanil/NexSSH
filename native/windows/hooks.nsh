!include "${__FILEDIR__}\..\..\desktop\windows\hooks.nsh"

; The Slint updater exits through its normal shutdown path: sessions, editors and settings
; finish before the installer checks for running applications or replaces the executable.
!macro NSIS_HOOK_PREINSTALL
  ${If} $UpdateMode = 1
    ${GetOptions} $CMDLINE "/NEXSSH_WAIT_PID=" $0
    ${IfNot} ${Errors}
      System::Call 'kernel32::OpenProcess(i 0x00100000, i 0, i r0) p .r1'
      ${If} $1 <> 0
        System::Call 'kernel32::WaitForSingleObject(p r1, i 60000) i .r2'
        System::Call 'kernel32::CloseHandle(p r1)'
        ${If} $2 <> 0
          DetailPrint "NexSSH did not finish shutting down; update cancelled."
          SetErrorLevel 1
          Abort
        ${EndIf}
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend
