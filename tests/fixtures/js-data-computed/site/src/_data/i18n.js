// The shape of a translation file with helper functions next to strings.
const strings = {
  en: { home: "Home", posts: "Posts" },
  es: { home: "Inicio", posts: "Entradas" },
};
module.exports = {
  en: {
    home: "Home",
    homeAriaLabel: (name) => `Go to the ${name} home page`,
  },
  t: (lang, key) => strings[lang][key],
  count: async (n) => `${n} item${n === 1 ? "" : "s"}`,
};
