#!/usr/bin/env python3
"""The installer's wording: Tauri's NSIS language files with our strings.

Usage: python tools/nsis-strings.py, after a Windows `npx tauri build`
(which writes Tauri's language files under target/release/nsis/x64).
"""
import json
import os
import re

TAURI = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'app', 'src-tauri')
SRC = os.path.join(TAURI, 'target', 'release', 'nsis', 'x64')
DST = os.path.join(TAURI, 'windows', 'lang')
CONF = os.path.join(TAURI, 'tauri.conf.json')

# Only the strings of the pages where an existing install meets a new one,
# and of the uninstaller's checkbox. Everything else is Tauri's text.
# "Your preferences" = browser, profiles, apps, file types (settings.json):
# kept in every case since 0.2.0 unless the box below is ticked.
OVERRIDES = {
    'French': {
        'alreadyInstalled': "Déjà installé.",
        'alreadyInstalledLong': "${PRODUCTNAME} ${VERSION} est déjà installé. Vos préférences (navigateur, apps, types de fichiers) sont conservées. Sélectionnez l'opération souhaitée, puis cliquez sur Suivant.",
        'olderOrUnknownVersionInstalled': "La version $R4 de ${PRODUCTNAME} est installée. Vos préférences (navigateur, apps, types de fichiers) sont conservées, quel que soit votre choix. Sélectionnez l'opération souhaitée, puis cliquez sur Suivant.",
        'newerVersionInstalled': "Une version plus récente de ${PRODUCTNAME} est déjà installée. Installer une version plus ancienne n'est pas recommandé. Sélectionnez l'opération souhaitée, puis cliquez sur Suivant.",
        'uninstallBeforeInstalling': "Remplacer l'ancienne version (recommandé)",
        'dontUninstall': "Installer par-dessus l'ancienne version",
        'addOrReinstall': "Réinstaller (vos préférences sont conservées).",
        'deleteAppData': "Supprimer aussi mes préférences (navigateur, apps, types de fichiers)",
    },
    'English': {
        'alreadyInstalledLong': "${PRODUCTNAME} ${VERSION} is already installed. Your preferences (browser, apps, file types) are kept. Select the operation you want to perform and click Next to continue.",
        'olderOrUnknownVersionInstalled': "Version $R4 of ${PRODUCTNAME} is installed. Your preferences (browser, apps, file types) are kept whatever you choose. Select the operation you want to perform and click Next to continue.",
        'uninstallBeforeInstalling': "Replace the old version (recommended)",
        'dontUninstall': "Install over the old version",
        'addOrReinstall': "Reinstall (your preferences are kept).",
        'deleteAppData': "Also delete my preferences (browser, apps, file types)",
    },
    'Spanish': {
        'alreadyInstalledLong': "${PRODUCTNAME} ${VERSION} ya está instalado. Sus preferencias (navegador, apps, tipos de archivo) se conservan. Seleccione la operación que desea realizar y haga clic en Siguiente para continuar.",
        'olderOrUnknownVersionInstalled': "La versión $R4 de ${PRODUCTNAME} está instalada. Sus preferencias (navegador, apps, tipos de archivo) se conservan sea cual sea su elección. Seleccione la operación que desea realizar y haga clic en Siguiente para continuar.",
        'uninstallBeforeInstalling': "Reemplazar la versión anterior (recomendado)",
        'dontUninstall': "Instalar sobre la versión anterior",
        'addOrReinstall': "Reinstalar (sus preferencias se conservan).",
        'deleteAppData': "Eliminar también mis preferencias (navegador, apps, tipos de archivo)",
    },
    'Japanese': {
        'alreadyInstalledLong': "${PRODUCTNAME} ${VERSION} はすでにインストールされています。設定（ブラウザー、アプリ、ファイルの種類）は保持されます。実行する操作を選択し、[次へ] をクリックしてください。",
        'olderOrUnknownVersionInstalled': "${PRODUCTNAME} のバージョン $R4 がインストールされています。どちらを選んでも、設定（ブラウザー、アプリ、ファイルの種類）は保持されます。実行する操作を選択し、[次へ] をクリックしてください。",
        'uninstallBeforeInstalling': "古いバージョンを置き換える（推奨）",
        'dontUninstall': "古いバージョンの上にインストールする",
        'addOrReinstall': "再インストール（設定は保持されます）。",
        'deleteAppData': "設定（ブラウザー、アプリ、ファイルの種類）も削除する",
    },
    'SimpChinese': {
        'alreadyInstalledLong': "${PRODUCTNAME} ${VERSION} 已安装。您的偏好设置（浏览器、应用、文件类型）会保留。请选择要执行的操作，然后点击“下一步”继续。",
        'olderOrUnknownVersionInstalled': "系统中已安装 ${PRODUCTNAME} $R4 版本。无论您如何选择，您的偏好设置（浏览器、应用、文件类型）都会保留。请选择要执行的操作，然后点击“下一步”继续。",
        'uninstallBeforeInstalling': "替换旧版本（推荐）",
        'dontUninstall': "直接安装在旧版本之上",
        'addOrReinstall': "重新安装（保留您的偏好设置）。",
        'deleteAppData': "同时删除我的偏好设置（浏览器、应用、文件类型）",
    },
    'TradChinese': {
        'alreadyInstalledLong': "${PRODUCTNAME} ${VERSION} 已安裝。您的偏好設定（瀏覽器、應用程式、檔案類型）會保留。請選擇要執行的操作，然後點擊「下一步」繼續。",
        'olderOrUnknownVersionInstalled': "系統中已安裝 ${PRODUCTNAME} $R4 版本。無論您如何選擇，您的偏好設定（瀏覽器、應用程式、檔案類型）都會保留。請選擇要執行的操作，然後點擊「下一步」繼續。",
        'uninstallBeforeInstalling': "取代舊版本（建議）",
        'dontUninstall': "直接安裝在舊版本之上",
        'addOrReinstall': "重新安裝（保留您的偏好設定）。",
        'deleteAppData': "同時刪除我的偏好設定（瀏覽器、應用程式、檔案類型）",
    },
    'Arabic': {
        'alreadyInstalledLong': "${PRODUCTNAME} ${VERSION} مثبّت بالفعل. تبقى تفضيلاتك (المتصفح، التطبيقات، أنواع الملفات) محفوظة. اختر العملية التي تريد تنفيذها، ثم انقر على التالي للمتابعة.",
        'olderOrUnknownVersionInstalled': "الإصدار $R4 من ${PRODUCTNAME} مثبّت. تبقى تفضيلاتك (المتصفح، التطبيقات، أنواع الملفات) محفوظة مهما كان اختيارك. اختر العملية التي تريد تنفيذها، ثم انقر على التالي للمتابعة.",
        'uninstallBeforeInstalling': "استبدال الإصدار القديم (موصى به)",
        'dontUninstall': "التثبيت فوق الإصدار القديم",
        'addOrReinstall': "إعادة التثبيت (تبقى تفضيلاتك محفوظة).",
        'deleteAppData': "حذف تفضيلاتي أيضًا (المتصفح، التطبيقات، أنواع الملفات)",
    },
}

HEADER = ("; Tauri's installer strings for this language (tauri-bundler, MIT or\n"
          "; Apache-2.0), with Kynoko Launcher's wording on the pages where an\n"
          "; installed version meets a new one, and on the uninstaller's checkbox:\n"
          "; users must see that their preferences are kept. Generated by\n"
          "; tools/nsis-strings.py from the files a Tauri build writes; a string Tauri\n"
          "; adds later is missing here until that script runs again.\n")

os.makedirs(DST, exist_ok=True)
files = {}
for lang, over in OVERRIDES.items():
    raw = open(os.path.join(SRC, f'{lang}.nsh'), 'rb').read()
    text = raw.decode('utf-8').lstrip('﻿')  # Tauri's BOM, or two of them
    seen = set()
    out = []
    for line in text.splitlines():
        line = line.lstrip('﻿')
        if line.startswith(';'):
            continue  # our own header, when a build copied our file back
        m = re.match(r'LangString (\w+) (\$\{LANG_\w+\}) "(.*)"$', line)
        if m and m.group(1) in over:
            out.append(f'LangString {m.group(1)} {m.group(2)} "{over[m.group(1)]}"')
            seen.add(m.group(1))
        else:
            out.append(line)
    missing = set(over) - seen
    assert not missing, (lang, missing)
    body = HEADER + '\n'.join(out) + '\n'
    # No BOM: Tauri adds one when it copies the file (two make makensis fail).
    open(os.path.join(DST, f'{lang}.nsh'), 'wb').write(body.encode('utf-8'))
    files[lang] = f'windows/lang/{lang}.nsh'

s = open(CONF, encoding='utf-8', newline='').read()
if '"customLanguageFiles"' in s:
    print('ok', list(files))
    raise SystemExit(0)
old = '        "installerHooks": "windows/hooks.nsh"'
assert s.count(old) == 1
entries = ',\n'.join(f'          "{k}": "{v}"' for k, v in files.items())
s = s.replace(old, '        "customLanguageFiles": {\n' + entries + '\n        },\n' + old)
json.loads(s)
open(CONF, 'w', encoding='utf-8', newline='').write(s)
print('ok', list(files))
