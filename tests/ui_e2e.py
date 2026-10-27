"""Run against an isolated RustLink process, never a user's configured gateway."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request
from playwright.sync_api import sync_playwright, expect

root=Path(__file__).resolve().parents[1]
with socket.socket() as sock:
    sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
with tempfile.TemporaryDirectory(prefix='rustlink-ui-') as directory:
    executable=root/('target/debug/rustlink.exe' if os.name=='nt' else 'target/debug/rustlink')
    process=subprocess.Popen([str(executable),'--headless','--strict-port','--port',str(port),'--data-dir',directory],creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0),stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
    try:
        for _ in range(100):
            try:urllib.request.urlopen(f'http://127.0.0.1:{port}/api/health',timeout=.2);break
            except Exception:time.sleep(.1)
        with sync_playwright() as p:
            existing=Path(os.environ.get('LOCALAPPDATA',''))/'ms-playwright/chromium_headless_shell-1234/chrome-headless-shell-win64/chrome-headless-shell.exe'
            browser=p.chromium.launch(headless=True,**({'executable_path':str(existing)} if existing.exists() else {}))
            page=browser.new_page(viewport={'width':1440,'height':1000})
            errors=[];page.on('pageerror',lambda error:errors.append(str(error)))
            page.goto(f'http://127.0.0.1:{port}/')
            page.get_by_role('button',name='设备配置',exact=True).click()
            page.get_by_role('button',name='添加设备',exact=True).click()
            page.locator('#dev-name').fill('模拟温度PLC')
            page.locator('#dev-ip').fill('127.0.0.1')
            page.get_by_role('button',name='添加点位',exact=True).click()
            page.locator('#point-0-name').fill('温度')
            page.locator('#point-0-address').fill('40001')
            page.get_by_role('button',name='保存设备',exact=True).click()
            page.get_by_role('cell',name='模拟温度PLC',exact=True).wait_for()
            page.get_by_role('button',name='编辑',exact=True).click()
            page.locator('#dev-name').fill('修改后的PLC')
            page.get_by_role('button',name='保存设备',exact=True).click()
            page.get_by_role('cell',name='修改后的PLC',exact=True).wait_for()
            page.get_by_role('button',name='系统设置',exact=True).click()
            page.locator('#collect-gateway_name').fill('测试车间')
            page.get_by_role('button',name='保存采集设置',exact=True).click()
            expect(page.locator('#notice')).to_have_text('配置已保存')
            page.reload();page.get_by_role('button',name='系统设置',exact=True).click()
            expect(page.locator('#collect-gateway_name')).to_have_value('测试车间')
            page.get_by_role('button',name='设备配置',exact=True).click()
            page.locator('#excel-file').set_input_files(str(root/'tests/fixtures/points_cn.xlsx'))
            page.get_by_role('button',name='确认导入',exact=True).click()
            assert page.locator('#point-0-name').input_value()=='温度'
            page.get_by_role('button',name='取消',exact=True).click()
            page.on('dialog',lambda dialog:dialog.accept())
            page.get_by_role('button',name='删除',exact=True).click()
            page.get_by_text('暂无设备，请先添加设备。',exact=True).wait_for()
            page.get_by_role('button',name='实时数据',exact=True).click()
            page.get_by_role('button',name='启动采集',exact=True).click()
            page.get_by_role('button',name='停止采集',exact=True).wait_for()
            page.get_by_role('button',name='停止采集',exact=True).click()
            page.get_by_role('button',name='启动采集',exact=True).wait_for()
            artifacts=root/'artifacts';artifacts.mkdir(exist_ok=True)
            page.screenshot(path=str(artifacts/'ui-home.png'),full_page=True)
            page.get_by_role('button',name='系统设置',exact=True).click()
            page.screenshot(path=str(artifacts/'ui-settings.png'),full_page=True)
            assert not errors,errors
            browser.close()
        print('PASS: Rust/WASM UI CRUD, point editing, persistence, Excel preview, start/stop; no browser exceptions')
    finally:
        process.terminate();process.wait(timeout=10)
