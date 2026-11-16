---
date: 2023-01-01
---
# Tags

{% for tag in collections.meta.tags -%}
- {{ tag }}
{% endfor %}

# Categories

{% for c in collections.meta.categories -%}
- {{ c }}
{% endfor %}

{% for c in collections.meta.posts.categories -%}
{{ c[0] }}={{ c[1] | size }}
{% endfor %}
