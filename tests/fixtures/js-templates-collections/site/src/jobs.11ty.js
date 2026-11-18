module.exports = class {
  data() {
    return {
      title: "Jobs",
      date: "2024-01-01",
      layout: "base.njk",
      pagination: { data: "collections.job", size: 2, alias: "jobs" },
      permalink: (d) => (d.pagination.pageNumber === 0 ? "/jobs/" : `/jobs/${d.pagination.pageNumber + 1}/`),
    };
  }
  render({ jobs, pagination }) {
    const items = jobs.map((j) => `<li><a href="${j.url}">${j.data.title}</a> ${j.date.toISOString().slice(0, 10)}</li>`);
    return `<ul>${items.join("")}</ul><p>${pagination.pageNumber + 1} of ${pagination.pages.length}</p>`;
  }
};
