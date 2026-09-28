// Shapes shared by the message catalogs (see ../i18n.svelte.ts).

/** Plural variants selected with Intl.PluralRules; `{count}` is the number. */
export type PluralForms = { other: string } & Partial<Record<Intl.LDMLPluralRule, string>>;

export type Messages = Record<string, string | PluralForms>;
