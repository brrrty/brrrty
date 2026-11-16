---
title: List
date: 2023-01-01
tags: listing
pagination:
  data: collections.post
  size: 2
permalink: "list/{{ pagination.pageNumber }}/"
---
{% for i in pagination.items %}{{ i.url }} {% endfor %}
