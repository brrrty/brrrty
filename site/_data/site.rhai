// Site-wide data. `git_head()` reads the commit this build comes from.
let commit = "";
try { commit = git_head().sub_string(0, 7); } catch {}

#{
    name: "Brrrty",
    url: "https://brrrty.com",
    description: "Brrrty builds your 11ty site in Rust and writes the same files 11ty 3.1.8 writes, byte for byte.",
    repo: "https://github.com/brrrty/brrrty",
    commit: commit,
    year: year(now()),
}
