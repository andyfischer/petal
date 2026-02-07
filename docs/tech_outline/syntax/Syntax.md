The syntax is not fully designed but here are some ideals and goals:

Inspirations: Rust, OCaml

# Let expressions

Bind a name to a result:

```
let a = 10
```

An existing name can be bound to a new expression:
```
let a = 10
a = 20
```

# Allowed identifiers

Function names can have `?` in them. These are often used for boolean functions. There is no `?:` ternary expression.
# C-Like Function Calls
```
print(1,2,3)
```

# Optional Commas
```
print(1 2 3)
```

# Persisted state
```
state key = 123
```

# Lists
```
let list = [1,2,3]
```

# Function Declaration

When defining a function, the last expression is implicitly used as the returned value.
```
fn square(n) {
    n * n
}
```

# Enums

Enums define a set of named variants:
```
enum Status { Active, Banned, Pending }
enum Role { Admin, User, Guest }
```

Variants are used as bare names (no prefix required):
```
let status = Active
let role = Admin
```

Name collisions between enum variants and local variables are a compile error. The compiler tracks which names are enum variants.

Enums with associated data (positional):
```
enum Result {
  Ok(value)
  Error(code, message)
}

let result = Ok(42)
let failure = Error(404, "Not found")
```

**Open question:** Should enum variants support named fields instead of/in addition to positional? e.g. `Error { code, message }` constructed as `Error { code: 404, message: "Not found" }`

# Records

Records use identifier keys:
```
let record = {
  name: "Name"
  description: "Description"
}

let user = {
  name: "Alice"
  status: Active
  role: Admin
}
```

Accessing fields:
```
user.name
user.status
```

# Pattern Matching

Basic matching with arrows:
```
let result = match status {
  Active -> "Active"
  Banned -> "Banned"
  _ -> "Other"
}
```

Destructuring lists:
```
match list {
  [] -> "empty"
  [x] -> "single element"
  [first, second] -> "two elements"
  [head, ...tail] -> "head is " ++ head
}
```

Destructuring records:
```
match user {
  { status: Active, role: Admin } -> "Active admin"
  { status: Active } -> "Active user"
  { name: n } -> "Name is " ++ n
}
```

Matching enums with data:
```
match result {
  Ok(value) -> "Got: " ++ value
  Error(code, msg) -> "Error " ++ code ++ ": " ++ msg
}
```

Guards with `if`:
```
match n {
  x if x < 0 -> "negative"
  x if x == 0 -> "zero"
  x -> "positive"
}
```

Nested destructuring:
```
match response {
  { status: Ok, data: { items: items } } -> process(items)
  { status: Error, code: 404 } -> "Not found"
  { status: Error, code: code, message: msg } -> "Error " ++ code ++ ": " ++ msg
}
```