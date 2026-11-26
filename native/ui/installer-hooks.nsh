; Tickmark 安装器的两个钩子：
; 1. 安装完成后把安装目录加入当前用户 PATH，新开的终端即可使用 tickmark。
; 2. 卸载前从用户 PATH 移除该目录；交互式卸载时单独询问是否删除使用记录。
; PATH 的读写由 tickmark path 完成，避免 NSIS 字符串长度限制和编码问题。

!macro NSIS_HOOK_POSTINSTALL
  ExecWait '"$INSTDIR\tickmark.exe" path --add "$INSTDIR"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\tickmark.exe" path --remove "$INSTDIR"'
  IfSilent tickmark_keep_records
  MessageBox MB_YESNO|MB_ICONQUESTION "是否同时删除本机的 Tickmark 使用记录？$\r$\n选择「否」将保留 $LOCALAPPDATA\TickmarkDesktop，重新安装后可继续查看历史。" IDNO tickmark_keep_records
  RMDir /r "$LOCALAPPDATA\TickmarkDesktop"
  tickmark_keep_records:
!macroend
