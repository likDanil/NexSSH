"""GPU hint regression: hover/leave cycles and enable notification, isolated data.
Build with SLINT_EMIT_DEBUG_INFO=1 and --features slint/mcp.
Run: uv run --with paramiko --with pillow native/tests/gpu_hint_ui.py
"""
import sys, os, json, socket, tempfile, subprocess, time
from pathlib import Path
sys.path.insert(0, str(Path('native/tests').resolve()))
from settings_ui import Inspector, wait_for
from terminal_fonts_ui import screenshot
with tempfile.TemporaryDirectory(prefix='nexssh-gpu-hint-') as folder:
    Path(folder,'settings.json').write_text(json.dumps({'language':'ru','gpuAcceleration':False,'autoUpdateCheck':False,'agentsEnabled':False,'explorerMenu':False,'restoreTabs':'never','closeAction':'quit'}),encoding='utf-8')
    with socket.socket() as s:
        s.bind(('127.0.0.1',0)); port=s.getsockname()[1]
    env=dict(os.environ,NEXSSH_DATA_DIR=folder,SLINT_MCP_PORT=str(port))
    env.pop('SLINT_BACKEND',None)
    p=subprocess.Popen([str(Path('target/debug/nexssh-slint.exe').resolve())],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,creationflags=subprocess.CREATE_NO_WINDOW)
    try:
        ui=Inspector(port)
        def hover(element):
            # Keep the native Windows cursor aligned with the injected event;
            # otherwise OS mouse motion can immediately overwrite synthetic hover.
            if os.name == 'nt':
                import ctypes
                from ctypes import wintypes
                user=ctypes.WinDLL('user32')
                user.FindWindowW.restype=wintypes.HWND
                user.FindWindowW.argtypes=[wintypes.LPCWSTR,wintypes.LPCWSTR]
                user.ClientToScreen.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.POINT)]
                bounds=ui.call('get_element_properties',{'elementHandle':element})
                pos=bounds['absolutePosition']; size=bounds['size']
                point=wintypes.POINT(round(pos['x']+size['width']/2),round(pos['y']+size['height']/2))
                user.ClientToScreen(user.FindWindowW(None,'NexSSH 2.0'),ctypes.byref(point))
                user.SetCursorPos(point.x,point.y)
                time.sleep(.1)
            ui.call('hover_element',{'elementHandle':element})
        ui.press('Настройки')
        ui.press('Терминал','Tab')
        hint='Отрисовка на GPU может повысить потребление оперативной памяти (ОЗУ). Изменение применяется после перезапуска приложения.'
        icon=ui.find(hint,'Groupbox')[0]
        for _ in range(3):
            hover(icon)
            wait_for(lambda:ui.find(hint,'Text'),'Hover tooltip missing')
            time.sleep(.3)
            assert ui.find(hint,'Text'), 'Tooltip flickers/disappears while hovered'
            hover(ui.find('Терминал','Tab')[0])
            wait_for(lambda:not ui.find(hint,'Text'),'Tooltip stays after pointer leaves')
        hover(icon)
        wait_for(lambda:ui.find(hint,'Text'),'Repeated hover missing')
        out=Path('target/gpu-hint');out.mkdir(exist_ok=True)
        (out/'tooltip.png').write_bytes(screenshot(ui))
        hover(ui.find('Терминал','Tab')[0])
        ui.press('Отрисовка на GPU','Switch')
        wait_for(lambda:ui.find('GPU включён: потребление ОЗУ может увеличиться. Настройка применится после перезапуска.','Text'),'Missing notice')
        (out/'enabled.png').write_bytes(screenshot(ui))
        print('PASS: 3 hover/leave cycles, stable hover, repeated hover and enable notification')
    finally:
        p.terminate();p.wait(timeout=5)
