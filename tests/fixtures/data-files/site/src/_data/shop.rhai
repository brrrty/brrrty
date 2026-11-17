// The port of shop.js.
let products = parse_csv(read_text("data/products.csv"), #{
    columns: true,
    skip_empty_lines: true,
    trim: true,
});
let notes = [];
for file in read_dir("data/notes") {
    if !file.ends_with(".md") {
        continue;
    }
    let raw = read_text("data/notes/" + file);
    // `[\s\S]` is `.` with the `s` flag.
    let pattern = "^---\\n(.*?)\\n---\\n?(.*)$";
    let has_front = regex_test(raw, pattern, "s");
    let front = if has_front { regex_replace(raw, pattern, "$1", "s") } else { "" };
    let body = if has_front { regex_replace(raw, pattern, "$2", "s") } else { raw };
    let title_pattern = "^.*?title:\\s*\"?([^\"\\n]+)\"?.*$";
    let note = object();
    note.file = file;
    note.title = if has_front && regex_test(front, "title:\\s*\"?([^\"\\n]+)\"?") {
        regex_replace(front, title_pattern, "$1", "s")
    } else {
        null_value()
    };
    // Rhai's `trim()` trims in place, as `body = body.trim()` would.
    body.trim();
    note.body = body;
    notes.push(note);
}
let missing = "none";
try {
    missing = read_text("data/missing.txt");
} catch {
    missing = "caught";
}
let shop = object();
shop.products = products;
shop.notes = notes;
shop.settings = read_json("data/settings.json");
shop.recipes = read_dir("data/recipes", #{ recursive: true });
shop.missing = missing;
shop.hasSettings = file_exists("data/settings.json");
shop.hasOther = file_exists("data/other.json");
shop.raw = parse_csv("a;b\n1;2", #{ delimiter: ";" });
shop
