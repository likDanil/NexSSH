// Interface language: message lookup, plurals and locale-aware formatting.
//
// Catalogs live in ./locales (English is the reference; TypeScript checks that other
// languages provide every key). t() reads reactive state, so templates re-render when
// the language changes. To add a language: add locales/xx.ts, register it in CATALOGS
// and LANGUAGES, and add its messages to core/src/i18n.rs for backend texts.

import en from './locales/en';
import ru from './locales/ru';
import type { PluralForms } from './locales/types';

export type Lang = 'en' | 'ru';
export type LanguageSetting = 'system' | Lang;

/** Languages offered in Settings, named in their own language. */
export const LANGUAGES: { id: Lang; name: string }[] = [
  { id: 'en', name: 'English' },
  { id: 'ru', name: 'Русский' },
];

type En = typeof en;
export type MessageKey = { [K in keyof En]: En[K] extends string ? K : never }[keyof En];
export type PluralKey = { [K in keyof En]: En[K] extends string ? never : K }[keyof En];
/** The shape every catalog must have: the English keys, plurals as full form sets. */
export type Catalog = { [K in keyof En]: En[K] extends string ? string : PluralForms };
export type Params = Record<string, string | number>;

const CATALOGS: Record<Lang, Catalog> = { en, ru };

class I18nState {
  lang = $state<Lang>('en');
}

export const i18n = new I18nState();

/** The first supported language in the OS/browser preferences, else English. */
export function systemLanguage(): Lang {
  const prefs = navigator.languages?.length ? navigator.languages : [navigator.language];
  for (const tag of prefs) {
    const base = tag?.toLowerCase().split(/[-_]/)[0];
    if (base && base in CATALOGS) return base as Lang;
  }
  return 'en';
}

export function resolveLanguage(setting: LanguageSetting | undefined, system: Lang): Lang {
  return setting && setting !== 'system' && setting in CATALOGS ? setting : system;
}

function fill(template: string, params?: Params): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (m, name: string) => (name in params ? String(params[name]) : m));
}

/** The message in the current language. */
export function t(key: MessageKey, params?: Params): string {
  return fill(CATALOGS[i18n.lang][key] as string, params);
}

/** The English message, e.g. as a search alias when the interface is translated. */
export function tEn(key: MessageKey, params?: Params): string {
  return fill(en[key], params);
}

const pluralRules = new Map<Lang, Intl.PluralRules>();

/** A message with a number: picks the plural form and fills `{count}`. */
export function tn(key: PluralKey, count: number, params?: Params): string {
  const lang = i18n.lang;
  let rules = pluralRules.get(lang);
  if (!rules) pluralRules.set(lang, (rules = new Intl.PluralRules(lang)));
  const forms = CATALOGS[lang][key] as PluralForms;
  return fill(forms[rules.select(count)] ?? forms.other, { ...params, count: formatNumber(count) });
}

export type Part = { text: string } | { param: string; value: string };

/** A message split around its placeholders, so templates can style the values. */
export function tParts(key: MessageKey, params: Params): Part[] {
  const template = CATALOGS[i18n.lang][key] as string;
  const parts: Part[] = [];
  let last = 0;
  for (const m of template.matchAll(/\{(\w+)\}/g)) {
    const index = m.index ?? 0;
    if (index > last) parts.push({ text: template.slice(last, index) });
    parts.push(m[1] in params ? { param: m[1], value: String(params[m[1]]) } : { text: m[0] });
    last = index + m[0].length;
  }
  if (last < template.length) parts.push({ text: template.slice(last) });
  return parts;
}

export function formatNumber(n: number): string {
  return n.toLocaleString(i18n.lang);
}

/** "5 min. ago", "вчера", or a date for older timestamps (seconds since the epoch). */
export function timeAgo(ts?: number): string {
  if (!ts) return '';
  const s = Math.max(0, Date.now() / 1000 - ts);
  if (s < 60) return t('time.justNow');
  const rtf = new Intl.RelativeTimeFormat(i18n.lang, { numeric: 'auto', style: 'short' });
  if (s < 3600) return rtf.format(-Math.floor(s / 60), 'minute');
  if (s < 86400) return rtf.format(-Math.floor(s / 3600), 'hour');
  if (s < 86400 * 30) return rtf.format(-Math.floor(s / 86400), 'day');
  return new Date(ts * 1000).toLocaleDateString(i18n.lang);
}
