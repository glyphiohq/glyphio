//! Dev helper: seed a store and render espanso YAML into a config dir.
//! Usage: cargo run --example gen -- <config_dir>

fn main() {
    let dir = std::env::args().nth(1).expect("usage: gen <config_dir>");
    let store = snippet_store::SnippetStore::open_in_memory().unwrap();
    store
        .create(snippet_store::NewSnippet {
            trigger: ":glyphio".into(),
            replacement: "Glyphio works! 🎉".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .create(snippet_store::NewSnippet {
            trigger: ":gdate".into(),
            replacement: "Today is {{gdate}}".into(),
            variables: Some(serde_json::json!([
                {"name": "gdate", "type": "date", "params": {"format": "%Y-%m-%d"}}
            ])),
            ..Default::default()
        })
        .unwrap();
    // Rich snippets (native espanso markdown/html injection).
    store
        .create(snippet_store::NewSnippet {
            trigger: ":gmd".into(),
            replacement: "**Bold** and _italic_ from Glyphio".into(),
            format: Some("markdown".into()),
            ..Default::default()
        })
        .unwrap();
    store
        .create(snippet_store::NewSnippet {
            trigger: ":ghtml".into(),
            replacement: "<b>Bold</b> <i>italic</i> <table><tr><td>a</td><td>b</td></tr></table>".into(),
            format: Some("html".into()),
            ..Default::default()
        })
        .unwrap();
    store.render_yaml(&dir).unwrap();
    println!("rendered {} snippets into {dir}", store.list().unwrap().len());
}
