---
date: 2023-01-03
pagination:
  data: collections.all
  size: 100
permalink: "every/"
---
{% for i in pagination.items %}{{ i.url }} {% endfor %}
