---
title: Fences
date: 2024-01-01
---

# Fences

A plain fence stays plain:

```
let plain = "<b>" & 1;
```

```js
// a comment
const greet = (name) => `Hello, ${name}!`;
if (a < b && c > d) { console.log("x"); }
```

```javascript
function add(a, b) {
  return a + b; /* sum */
}
```

~~~php
<?php
$name = "World & <you>";
echo "Hello, {$name}";
?>
<p>html</p>
~~~

~~~~ruby
def hi = puts "hi"
~~~~

````markdown
# Title

Some *emphasis*, a [link](https://example.com) and `code`.

```
a fence with no language
```
````

```js {1} title="x"
let infoWithWords = 1;
```

```python extra words here
print("hi")
```

```js
```

```text
<b>Not escaped</b> & raw
```

```foo
<i>unknown</i> & <raw>
```

```TEXT
<u>case</u>
```

- A list item

  ```bash
  echo "$HOME" | grep -v x > /dev/null
  ```

- Another

  ~~~shell
  ls -la ~
  ~~~

> A quote
>
> ```css
> a:hover { color: #fff; }
> ```

1. Ordered

   > ```sh
   > export A=1
   > ```

```html
<!DOCTYPE html>
<a href="#x" title="&amp;">Link &copy;</a>
```

```xml
<?xml version="1.0"?><root a="1"/>
```

```json
{ "a": [1, 2.5, true, null], "b": "c" }
```

```yaml
key: value
list:
  - one # comment
```

```ts
type A = { a: number };
const x: A = { a: 1 };
```

```sql
SELECT * FROM users WHERE id = 1; -- hi
```

```diff
- old line
+ new line
  same line
```

```diff-js
- const a = 1;
+ const a = 2;
  const b = "&";
```

```diff-foo
- removed <b>
+ added
```

```jsx
const el = <div className="x">Hello {name}</div>;
```

```less
@c: #fff;
.a { color: @c; }
```

```go
package main
func main() { println("hi") }
```

```rust
fn main() { let v: Vec<u8> = vec![]; }
```

```ini
[section]
key=value ; c
```

```nginx
server { listen 80; }
```

```console
$ echo hi
```

```c
#include <stdio.h>
int main(void) { return 0; }
```
