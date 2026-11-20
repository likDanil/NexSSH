//! Keep the Svelte 1.x design sources authoritative during the migration.
use std::{collections::BTreeMap, env, fs, path::PathBuf};
#[path = "build/settings.rs"]
mod settings;

fn block<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .expect(start)
        .1
        .split_once(end)
        .expect(end)
        .0
}

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    settings::generate(&out);
    let css_path = "../ui/src/app.css";
    let icons_path = "../ui/src/lib/components/Icon.svelte";
    let palettes_path = "../ui/src/lib/themes.ts";
    for path in [css_path, icons_path, palettes_path] {
        println!("cargo:rerun-if-changed={path}");
    }
    let css = fs::read_to_string(css_path).unwrap();
    let names = ["light", "graphite", "black", "navy"];
    let colors: Vec<BTreeMap<&str, &str>> = names
        .iter()
        .map(|name| {
            let marker = format!(":root[data-theme='{name}'] {{");
            block(&css, &marker, "}")
                .lines()
                .filter_map(|line| {
                    let (name, value) = line.trim().strip_prefix("--")?.split_once(':')?;
                    let value = value.trim().trim_end_matches(';');
                    (value.starts_with('#') || value.starts_with("rgba(")).then_some((name, value))
                })
                .collect()
        })
        .collect();
    let mut theme = String::from(
        "// Generated from ui/src/app.css. Edit the original design tokens.\nexport global Theme {\n    in property <int> variant: 0;\n",
    );
    for key in colors[0].keys() {
        theme.push_str(&format!("    out property <color> {key}: "));
        for (i, palette) in colors.iter().enumerate().skip(1) {
            theme.push_str(&format!(
                "variant == {i} ? {} : ",
                palette.get(key).unwrap()
            ));
        }
        theme.push_str(&format!("{};\n", colors[0][key]));
    }
    theme.push_str("}\n");
    fs::write(out.join("theme.slint"), theme).unwrap();

    let source = fs::read_to_string(icons_path).unwrap();
    let icons: Vec<_> = block(&source, "const ICONS: Record<string, string> = {", "  };")
        .lines()
        .filter_map(|line| {
            let (name, svg) = line.trim().split_once(": '")?;
            Some((name, svg.strip_suffix("',")?))
        })
        .collect();
    fs::create_dir_all(out.join("icons")).unwrap();
    let mut component = String::from(
        "// SVG paths from ui/src/lib/components/Icon.svelte.\nexport component Icon inherits Image {\n    in property <string> name;\n    width: 16px; height: 16px;\n    source: ",
    );
    for (name, contents) in &icons {
        let stroke = if name.starts_with("win") {
            "1.2"
        } else {
            "1.7"
        };
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"black\" stroke-width=\"{stroke}\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{}</svg>",
            contents.replace("currentColor", "black")
        );
        fs::write(out.join("icons").join(format!("{name}.svg")), svg).unwrap();
        component.push_str(&format!(
            "name == \"{name}\" ? @image-url(\"icons/{name}.svg\") : "
        ));
    }
    component.push_str("@image-url(\"icons/plus.svg\");\n}\n");
    fs::write(out.join("icons.slint"), component).unwrap();

    let source = fs::read_to_string(palettes_path).unwrap();
    let locale_path = "../ui/src/lib/locales/ru.ts";
    println!("cargo:rerun-if-changed={locale_path}");
    let locale = fs::read_to_string(locale_path).unwrap();
    let mut catalog = String::from(
        "\nexport struct ThemePreview { name: string, frame: color, surface: color, text: color, accent: color }\nexport global ThemeCatalog {\n    out property <[ThemePreview]> entries: [\n",
    );
    for line in block(&source, "export const THEMES: ThemeMeta[] = [", "];").lines() {
        let Some(rest) = line.trim().strip_prefix("{ id: '") else {
            continue;
        };
        let (id, rest) = rest.split_once("', preview: [").unwrap();
        let values: Vec<_> = rest
            .split(']')
            .next()
            .unwrap()
            .split(',')
            .map(|v| v.trim().trim_matches('\''))
            .collect();
        assert_eq!(values.len(), 4, "theme preview must contain four colors");
        let label = block(&locale, &format!("'theme.{id}': '"), "',");
        catalog.push_str(&format!(
            "        {{ name: \"{label}\", frame: {}, surface: {}, text: {}, accent: {} }},\n",
            values[0], values[1], values[2], values[3]
        ));
    }
    catalog.push_str("    ];\n}\n");
    let mut theme = fs::read_to_string(out.join("theme.slint")).unwrap();
    theme.push_str(&catalog);
    fs::write(out.join("theme.slint"), theme).unwrap();
    let parse = |name: &str| -> BTreeMap<String, String> {
        let marker = if name == "darkAnsi" {
            "const darkAnsi = {".to_owned()
        } else {
            format!("const {name}: ITheme = {{")
        };
        block(&source, &marker, "};")
            .lines()
            .filter_map(|line| {
                let (key, value) = line.trim().split_once(": '#")?;
                Some((key.to_owned(), value.strip_suffix("',")?.to_owned()))
            })
            .collect()
    };
    let dark = parse("darkAnsi");
    let keys = [
        "background",
        "foreground",
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "white",
        "brightBlack",
        "brightRed",
        "brightGreen",
        "brightYellow",
        "brightBlue",
        "brightMagenta",
        "brightCyan",
        "brightWhite",
    ];
    let mut rust = String::from(
        "// Generated from ui/src/lib/themes.ts.\npub const PALETTES: [[u32; 18]; 4] = [\n",
    );
    for name in names {
        let mut palette = if name == "graphite" || name == "black" {
            dark.clone()
        } else {
            BTreeMap::new()
        };
        palette.extend(parse(name));
        rust.push_str("    [");
        for key in keys {
            rust.push_str(&format!("0x{},", palette[key]));
        }
        rust.push_str("],\n");
    }
    rust.push_str("];\n");
    fs::write(out.join("terminal_palettes.rs"), rust).unwrap();
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_include_paths(vec![out]),
    )
    .expect("cannot compile the native UI");
}
