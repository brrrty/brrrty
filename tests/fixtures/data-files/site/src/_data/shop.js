// Reads files the way a typical data file does: a CSV with csv-parse, a
// folder of Markdown notes, a JSON file and a folder tree.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { parse } from 'csv-parse/sync';

export default function () {
  const products = parse(readFileSync('data/products.csv', 'utf8'), {
    columns: true,
    skip_empty_lines: true,
    trim: true,
  });
  const notes = readdirSync('data/notes')
    .filter((f) => f.endsWith('.md'))
    .map((file) => {
      const raw = readFileSync(`data/notes/${file}`, 'utf8');
      const m = raw.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
      const title = m ? m[1].match(/title:\s*"?([^"\n]+)"?/) : null;
      return { file, title: title ? title[1] : null, body: (m ? m[2] : raw).trim() };
    });
  const settings = JSON.parse(readFileSync('data/settings.json', 'utf8'));
  const recipes = readdirSync('data/recipes', { recursive: true });
  let missing = 'none';
  try {
    missing = readFileSync('data/missing.txt', 'utf8');
  } catch {
    missing = 'caught';
  }
  return {
    products,
    notes,
    settings,
    recipes,
    missing,
    hasSettings: existsSync('data/settings.json'),
    hasOther: existsSync('data/other.json'),
    raw: parse('a;b\n1;2', { delimiter: ';' }),
  };
}
