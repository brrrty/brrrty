---
layout: layouts/post
title: First Post
date: 2026-03-04T12:00:00Z
tags: [first]
author: Page Author
---
Hello from **{{ title }}** at {{ page.url }}.

{% raw %}Liquid stays as written here: {{ not_rendered }} and {% if x %}.{% endraw %}

```liquid
{% raw %}{% for post in collections.posts %}{{ post.data.title }}{% endfor %}{% endraw %}
```

{% include "callout" %}

{% include "fragment.md" %}

- templateSyntax: {{ page.templateSyntax }}
- fileSlug: {{ page.fileSlug }}
- filePathStem: {{ page.filePathStem }}
