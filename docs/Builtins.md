# Builtins Reference

All built-in functions available in Petal.

## I/O

### `print(args...)`

Prints arguments to stdout, separated by spaces, followed by a newline.

```petal
print("hello")           // hello
print(1, "and", 2)       // 1 and 2
print([1, 2], {a: 3})    // [1, 2] {a: 3}
```

## Math

### `abs(x)`

Returns the absolute value.

```petal
abs(-5)    // 5
abs(3.2)   // 3.2
```

### `sqrt(x)`

Returns the square root.

```petal
sqrt(9.0)   // 3.0
sqrt(2.0)   // 1.4142135623730951
```

### `floor(x)`

Rounds down to the nearest integer (returns float).

```petal
floor(3.7)   // 3.0
floor(-1.2)  // -2.0
```

### `ceil(x)`

Rounds up to the nearest integer (returns float).

```petal
ceil(3.2)   // 4.0
ceil(-1.7)  // -1.0
```

### `round(x)`

Rounds to the nearest integer (returns float).

```petal
round(3.4)   // 3.0
round(3.6)   // 4.0
```

### `min(a, b)`

Returns the smaller of two values. Works with numbers and strings.

```petal
min(3, 5)       // 3
min("a", "b")   // "a"
```

### `max(a, b)`

Returns the larger of two values. Works with numbers and strings.

```petal
max(3, 5)       // 5
max("a", "b")   // "b"
```

### `random(min, max)`

Returns a pseudo-random float in the range [min, max).

```petal
random(0.0, 1.0)    // 0.7342... (varies)
random(1.0, 10.0)   // 4.218...  (varies)
```

### `range(start, end)`

Returns a list of integers from `start` (inclusive) to `end` (exclusive).

```petal
range(0, 5)    // [0, 1, 2, 3, 4]
range(3, 7)    // [3, 4, 5, 6]
```

## Type Conversion

### `str(value)`

Converts any value to its string representation.

```petal
str(42)        // "42"
str(true)      // "true"
str([1, 2])    // "[1, 2]"
```

### `int(value)`

Converts to an integer. Accepts numbers and numeric strings.

```petal
int(3.7)     // 3
int("42")    // 42
```

### `float(value)`

Converts to a float.

```petal
float(42)    // 42.0
```

### `type(value)`

Returns the type name as a string.

```petal
type(42)          // "int"
type(3.14)        // "float"
type("hello")     // "string"
type([1, 2])      // "list"
type({a: 1})      // "map"
type(true)        // "bool"
type(nil)         // "nil"
```

## Collections

### `len(collection)`

Returns the length of a list or string.

```petal
len([1, 2, 3])   // 3
len("hello")     // 5
len([])          // 0
```

### `push(list, value)`

Appends a value to the end of a list. Mutates the list in place.

```petal
let items = [1, 2]
push(items, 3)     // items is now [1, 2, 3]
```

### `append(list, value)`

Same as `push` — appends a value to the end of a list.

### `pop(list)`

Removes and returns the last element. Returns `nil` if the list is empty.

```petal
let items = [1, 2, 3]
let last = pop(items)   // last = 3, items = [1, 2]
```

### `keys(record)`

Returns a list of all keys from a record.

```petal
keys({name: "Alice", age: 30})   // ["name", "age"]
```

### `values(record)`

Returns a list of all values from a record.

```petal
values({a: 1, b: 2})   // [1, 2]
```

### `contains(collection, needle)`

Checks if a list contains a value or a string contains a substring.

```petal
contains([1, 2, 3], 2)       // true
contains("hello", "ell")     // true
contains([1, 2, 3], 5)       // false
```

### `sort(list)`

Returns a new sorted list. Numbers sort before strings.

```petal
sort([3, 1, 2])           // [1, 2, 3]
sort(["c", "a", "b"])     // ["a", "b", "c"]
```

### `reverse(collection)`

Returns a new reversed list or string.

```petal
reverse([1, 2, 3])    // [3, 2, 1]
reverse("hello")      // "olleh"
```

### `join(list, separator)`

Joins list elements into a string with the given separator.

```petal
join(["a", "b", "c"], ", ")   // "a, b, c"
join([1, 2, 3], "-")          // "1-2-3"
```

### `split(string, separator)`

Splits a string into a list by the given separator.

```petal
split("a,b,c", ",")     // ["a", "b", "c"]
split("hello", "")       // ["h", "e", "l", "l", "o"]
```

### `enumerate(list)`

Returns a list of `[index, value]` pairs.

```petal
enumerate(["a", "b", "c"])   // [[0, "a"], [1, "b"], [2, "c"]]
```

### `zip(list_a, list_b)`

Pairs elements from two lists. Stops at the shorter list.

```petal
zip([1, 2], ["a", "b"])   // [[1, "a"], [2, "b"]]
```

### `slice(collection, start, end?)`

Returns a slice of a list or string. Supports negative indices. `end` defaults to the
length of the collection.

```petal
slice([1, 2, 3, 4], 1, 3)    // [2, 3]
slice([1, 2, 3, 4], -2)      // [3, 4]
slice("hello", 1, 3)         // "el"
```

### `flat(list)`

Flattens one level of nesting.

```petal
flat([[1, 2], [3, 4]])       // [1, 2, 3, 4]
flat([[1, [2]], [3]])         // [1, [2], 3]
```

## Higher-Order Functions

### `map(list, fn)`

Applies a function to each element and returns a new list.

```petal
map([1, 2, 3], fn(x) { x * 2 })        // [2, 4, 6]
map(["a", "b"], fn(s) { s ++ "!" })     // ["a!", "b!"]
```

### `filter(list, fn)`

Returns a new list containing only elements where the function returns `true`.

```petal
filter([1, 2, 3, 4], fn(x) { x > 2 })       // [3, 4]
filter(["hi", "", "ok"], fn(s) { len(s) > 0 })  // ["hi", "ok"]
```

### `reduce(list, initial, fn)`

Folds over a list, accumulating a result.

```petal
reduce([1, 2, 3], 0, fn(acc, x) { acc + x })   // 6
reduce([1, 2, 3], 1, fn(acc, x) { acc * x })   // 6
```

## Automatic Differentiation

These functions support forward-mode automatic differentiation with dual numbers.

### `dual(value, derivative)`

Creates a dual number with the given primal value and derivative.

```petal
let x = dual(3.0, 1.0)   // value = 3.0, derivative = 1.0
```

### `value_of(x)`

Extracts the primal value from a dual number. Returns a float for regular numbers.

```petal
value_of(dual(3.0, 1.0))   // 3.0
value_of(42)                // 42.0
```

### `deriv_of(x)`

Extracts the derivative from a dual number. Returns `0.0` for regular numbers.

```petal
deriv_of(dual(3.0, 1.0))   // 1.0
deriv_of(42)                // 0.0
```
