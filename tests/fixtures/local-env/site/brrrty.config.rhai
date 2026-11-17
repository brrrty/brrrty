// Brrrty's port of eleventy.config.js.
config.add_filter("localParts", |args| {
    let d = date(args[0], args[1], args[2], args[3], args[4]);
    [d.get_full_year(), d.get_month(), d.get_date(), d.get_day(), d.get_hours(), d.get_minutes(),
        d.get_timezone_offset(), d.to_iso_string()].map(|x| js_string(x)).reduce(|a, b| a + " " + b)
});
config.add_filter("num", |n| to_locale_string(n));
config.add_filter("enNum", |n| to_locale_string(n, "en-US"));
config.add_filter("sortLocale", |arr| {
    let a = [];
    for x in arr { a.push(x); }
    a.sort(|x, y| locale_compare(x, y));
    a.reduce(|p, q| p + " " + q)
});
config.add_filter("sortEn", |arr| {
    let a = [];
    for x in arr { a.push(x); }
    a.sort(|x, y| locale_compare(x, y, "en-US"));
    a.reduce(|p, q| p + " " + q)
});
#{ dir: #{ input: "src" } }
