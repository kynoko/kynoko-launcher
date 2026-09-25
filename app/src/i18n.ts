/**
 * The window's words, in the seven Kynoko languages. French is the source.
 * No em dash anywhere (house rule for every Kynoko translation).
 */
export type Lang = 'fr' | 'en' | 'es' | 'ja' | 'zh-Hans' | 'zh-Hant' | 'ar';

type Dict = Record<
  | 'LEAD' | 'BROWSER_TITLE' | 'DEFAULT_BROWSER' | 'SYSTEM_DEFAULT' | 'FOLLOW_DEFAULT' | 'NO_BROWSERS'
  | 'APPS_TITLE' | 'OPEN_FILES' | 'OPEN_APP' | 'BROWSER_FOR' | 'DEFAULT_APPS_NOTE' | 'DEFAULT_APPS_BUTTON'
  | 'REMOVE_ALL' | 'REMOVE_CONFIRM' | 'REMOVED' | 'CATALOGUE',
  string
>;

const FR: Dict = {
  LEAD: "Vos fichiers s'ouvrent dans les apps Kynoko, dans le navigateur de votre choix, et s'enregistrent à leur place.",
  BROWSER_TITLE: 'Navigateur',
  DEFAULT_BROWSER: 'Navigateur par défaut',
  SYSTEM_DEFAULT: 'Celui du système',
  FOLLOW_DEFAULT: 'Navigateur par défaut',
  NO_BROWSERS: 'Aucun navigateur trouvé : celui du système sera utilisé.',
  APPS_TITLE: 'Applications',
  OPEN_FILES: 'Ouvrir leurs fichiers',
  OPEN_APP: 'Ouvrir',
  BROWSER_FOR: 'Navigateur de {{app}}',
  DEFAULT_APPS_NOTE: "Windows ne laisse aucun programme se choisir lui-même par défaut : dans les paramètres, choisissez Kynoko Launcher pour les types de fichiers voulus.",
  DEFAULT_APPS_BUTTON: 'Ouvrir les applications par défaut',
  REMOVE_ALL: 'Tout retirer',
  REMOVE_CONFIRM: "Retirer toutes les associations de fichiers et tous les réglages de Kynoko Launcher ?",
  REMOVED: 'Tout est retiré.',
  CATALOGUE: 'Catalogue du {{date}}',
};

const EN: Dict = {
  LEAD: 'Your files open in the Kynoko apps, in the browser you choose, and save back in place.',
  BROWSER_TITLE: 'Browser',
  DEFAULT_BROWSER: 'Default browser',
  SYSTEM_DEFAULT: "The system's",
  FOLLOW_DEFAULT: 'Default browser',
  NO_BROWSERS: "No browser found: the system's will be used.",
  APPS_TITLE: 'Apps',
  OPEN_FILES: 'Open their files',
  OPEN_APP: 'Open',
  BROWSER_FOR: 'Browser for {{app}}',
  DEFAULT_APPS_NOTE: 'Windows lets no program make itself the default: in Settings, choose Kynoko Launcher for the file types you want.',
  DEFAULT_APPS_BUTTON: 'Open default apps',
  REMOVE_ALL: 'Remove everything',
  REMOVE_CONFIRM: 'Remove every file association and every setting of Kynoko Launcher?',
  REMOVED: 'Everything is removed.',
  CATALOGUE: 'Catalogue of {{date}}',
};

const ES: Dict = {
  LEAD: 'Tus archivos se abren en las apps de Kynoko, en el navegador que elijas, y se guardan en su sitio.',
  BROWSER_TITLE: 'Navegador',
  DEFAULT_BROWSER: 'Navegador predeterminado',
  SYSTEM_DEFAULT: 'El del sistema',
  FOLLOW_DEFAULT: 'Navegador predeterminado',
  NO_BROWSERS: 'No se ha encontrado ningún navegador: se usará el del sistema.',
  APPS_TITLE: 'Aplicaciones',
  OPEN_FILES: 'Abrir sus archivos',
  OPEN_APP: 'Abrir',
  BROWSER_FOR: 'Navegador de {{app}}',
  DEFAULT_APPS_NOTE: 'Windows no deja que ningún programa se elija a sí mismo como predeterminado: en Configuración, elige Kynoko Launcher para los tipos de archivo que quieras.',
  DEFAULT_APPS_BUTTON: 'Abrir aplicaciones predeterminadas',
  REMOVE_ALL: 'Quitarlo todo',
  REMOVE_CONFIRM: '¿Quitar todas las asociaciones de archivos y todos los ajustes de Kynoko Launcher?',
  REMOVED: 'Todo se ha quitado.',
  CATALOGUE: 'Catálogo del {{date}}',
};

const JA: Dict = {
  LEAD: 'ファイルは選んだブラウザーで Kynoko のアプリで開き、元の場所に保存されます。',
  BROWSER_TITLE: 'ブラウザー',
  DEFAULT_BROWSER: '既定のブラウザー',
  SYSTEM_DEFAULT: 'システムの既定',
  FOLLOW_DEFAULT: '既定のブラウザー',
  NO_BROWSERS: 'ブラウザーが見つかりません。システムの既定を使います。',
  APPS_TITLE: 'アプリ',
  OPEN_FILES: 'ファイルを開く',
  OPEN_APP: '開く',
  BROWSER_FOR: '{{app}} のブラウザー',
  DEFAULT_APPS_NOTE: 'Windows ではプログラムが自分自身を既定に設定できません。設定で、必要なファイルの種類に Kynoko Launcher を選んでください。',
  DEFAULT_APPS_BUTTON: '既定のアプリを開く',
  REMOVE_ALL: 'すべて削除',
  REMOVE_CONFIRM: 'Kynoko Launcher のファイルの関連付けと設定をすべて削除しますか？',
  REMOVED: 'すべて削除しました。',
  CATALOGUE: '{{date}} のカタログ',
};

const ZH_HANS: Dict = {
  LEAD: '您的文件会在您选择的浏览器中用 Kynoko 应用打开，并保存回原处。',
  BROWSER_TITLE: '浏览器',
  DEFAULT_BROWSER: '默认浏览器',
  SYSTEM_DEFAULT: '系统默认',
  FOLLOW_DEFAULT: '默认浏览器',
  NO_BROWSERS: '未找到浏览器：将使用系统默认浏览器。',
  APPS_TITLE: '应用',
  OPEN_FILES: '打开其文件',
  OPEN_APP: '打开',
  BROWSER_FOR: '{{app}} 的浏览器',
  DEFAULT_APPS_NOTE: 'Windows 不允许程序将自己设为默认：请在设置中为所需的文件类型选择 Kynoko Launcher。',
  DEFAULT_APPS_BUTTON: '打开默认应用',
  REMOVE_ALL: '全部移除',
  REMOVE_CONFIRM: '移除 Kynoko Launcher 的所有文件关联和所有设置？',
  REMOVED: '已全部移除。',
  CATALOGUE: '{{date}} 的目录',
};

const ZH_HANT: Dict = {
  LEAD: '您的檔案會在您選擇的瀏覽器中以 Kynoko 應用程式開啟，並儲存回原處。',
  BROWSER_TITLE: '瀏覽器',
  DEFAULT_BROWSER: '預設瀏覽器',
  SYSTEM_DEFAULT: '系統預設',
  FOLLOW_DEFAULT: '預設瀏覽器',
  NO_BROWSERS: '找不到瀏覽器：將使用系統預設瀏覽器。',
  APPS_TITLE: '應用程式',
  OPEN_FILES: '開啟其檔案',
  OPEN_APP: '開啟',
  BROWSER_FOR: '{{app}} 的瀏覽器',
  DEFAULT_APPS_NOTE: 'Windows 不允許程式將自己設為預設：請在設定中為所需的檔案類型選擇 Kynoko Launcher。',
  DEFAULT_APPS_BUTTON: '開啟預設應用程式',
  REMOVE_ALL: '全部移除',
  REMOVE_CONFIRM: '移除 Kynoko Launcher 的所有檔案關聯和所有設定？',
  REMOVED: '已全部移除。',
  CATALOGUE: '{{date}} 的目錄',
};

const AR: Dict = {
  LEAD: 'تُفتح ملفاتك في تطبيقات Kynoko، في المتصفح الذي تختاره، وتُحفظ في مكانها.',
  BROWSER_TITLE: 'المتصفح',
  DEFAULT_BROWSER: 'المتصفح الافتراضي',
  SYSTEM_DEFAULT: 'متصفح النظام',
  FOLLOW_DEFAULT: 'المتصفح الافتراضي',
  NO_BROWSERS: 'لم يُعثر على أي متصفح: سيُستخدم متصفح النظام.',
  APPS_TITLE: 'التطبيقات',
  OPEN_FILES: 'فتح ملفاتها',
  OPEN_APP: 'فتح',
  BROWSER_FOR: 'متصفح {{app}}',
  DEFAULT_APPS_NOTE: 'لا يسمح Windows لأي برنامج بأن يجعل نفسه افتراضيًا: في الإعدادات، اختر Kynoko Launcher لأنواع الملفات التي تريدها.',
  DEFAULT_APPS_BUTTON: 'فتح التطبيقات الافتراضية',
  REMOVE_ALL: 'إزالة كل شيء',
  REMOVE_CONFIRM: 'هل تريد إزالة جميع اقترانات الملفات وجميع إعدادات Kynoko Launcher؟',
  REMOVED: 'تمت إزالة كل شيء.',
  CATALOGUE: 'كتالوج {{date}}',
};

const DICTS: Record<Lang, Dict> = { fr: FR, en: EN, es: ES, ja: JA, 'zh-Hans': ZH_HANS, 'zh-Hant': ZH_HANT, ar: AR };

/** The system's language among the seven, English otherwise. */
export function pickLang(tags: readonly string[]): Lang {
  for (const tag of tags) {
    const t = tag.toLowerCase();
    if (t.startsWith('zh')) return /hant|tw|hk|mo/.test(t) ? 'zh-Hant' : 'zh-Hans';
    const base = t.split('-')[0];
    if (base in DICTS) return base as Lang;
  }
  return 'en';
}

export function t(lang: Lang, key: keyof Dict, params: Record<string, string> = {}): string {
  return DICTS[lang][key].replace(/\{\{(\w+)\}\}/g, (_, k: string) => params[k] ?? '');
}
