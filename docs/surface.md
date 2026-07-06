# Surface Syntax

This note records the current surface syntax direction. It is a working
frontend spec, not the semantic core.

## Items

Top-level items are type aliases, structs, enums, globals, functions, impls,
and traits.

```linear
type UserId = u32

struct User { id: UserId, balance: u32 }
struct MyInt(u32)

enum Option<T> {
  none,
  some(T),
}

global root: User

fn id<T>(x: T) -> T {
  x
}

trait Eq {
  fn eq(self: Self, other: Self) -> Bool
}

impl Eq for User {
  fn eq(self, other: User) -> Bool {
    self.id == other.id
  }
}
```

`struct MyInt(u32)` is valid. `struct MyInt u32` is not. Tuple and record
syntax must use parentheses or braces so the grouping is explicit.

## Grouping

Braces are used for named products and blocks:

```linear
struct User { id: UserId, balance: u32 }

{
  let one = 1
  x + one
}
```

Parentheses are used for calls, parameter lists, grouping, and positional
tuples:

```linear
some(value)
(left, right)
```

## Types

Named generic types use angle brackets:

```linear
HashMap<UserId, User>
```

Named products use braces. Positional products use parentheses. Function types
use `->`.

```linear
{ users: Users, events: Events }
(u32, Bool)
Request -> Decision
```

Enums have lowercase variant names. Variants may carry no payload, a positional
payload, or a named record payload.

```linear
enum Decision {
  allow { reason: u32 },
  deny { reason: u32 },
  review { queue: u32, priority: u32 },
}
```

## Expressions

Function calls have ordered arguments. Labels are optional documentation sugar
at the call site; they do not reorder arguments.

```linear
insert(users, key: id, @value: user)
```

Method syntax is sugar for calling a function with `self` as the first
argument.

```linear
users.insert!(key: id, @value: user)
```

Constructors use the type or variant as a callable value.

```linear
User { id: id, balance: 0 }
Decision.allow { reason: 0 }
Option.some(value)
```

`if ... else if ... else` parses as nested `if`.

```linear
if x < 10 {
  0
} else if x < 20 {
  1
} else {
  2
}
```

`match` arms may use `:` or `=>` as the arm separator.

```linear
match decision {
  .allow { reason }: reason,
  .review { queue, priority: p }: p,
}
```

## Patterns

`let` and match payloads accept patterns:

```linear
let x = value
let _ = ignored
let (left, right) = pair
let { users, events: next_events } = state
```

Patterns are frontend syntax. HIR/lowering is responsible for turning them into
explicit projection/split operations.

## Current Type Lowering

The first semantic lowering pass handles non-generic type items:

- `type` aliases;
- nominal `struct` products;
- nominal `enum` sums;
- builtin finite types `u8`, `u16`, `u32`, and `u64`;
- builtin `Bool`, `Symbol`, and `Text`;
- collection families `List`, `Vector`/`Vec`, `MutList`/`MutVector`, `HashMap`,
  and `MutHashMap`.

Generic declarations are parsed but rejected by this pass until monomorphization
or another generic strategy exists.

The signature lowering pass builds on this and registers:

- global declarations;
- free function parameter and output types;
- inherent impl methods, with `self` expanded to the impl target type;
- trait impl methods as named functions, without lowering trait semantics yet.

The resulting `CoreProgram` is a name/type skeleton. Function bodies are kept as
frontend AST for the later body-lowering pass, so the skeleton is not expected
to pass core checking yet.

The first body-lowering pass supports a deliberately small executable subset:

- function parameters and local names;
- `let name = expr` and typed scalar lets;
- integer literals;
- direct calls to known functions;
- references to declared globals;
- product constructors such as `Pair { left: x, right: y }`;
- finite `+`, `-`, `*`, `==`, `<`, and `>` over integer-like finite types.

It lowers free functions and impl methods into checked core functions. Complex
patterns, field access, methods at call sites, `if`, `match`, string literals,
enum constructors, and collection operations are still intentionally rejected
until their lowering rules are added.

## Value Flow Markers

At function definitions and call sites:

- no marker: the argument is returned unchanged;
- `!`: the argument is returned changed;
- `@`: the argument is consumed and not returned.

Markers can appear before parameters, before call arguments, and after a method
name to mark the receiver:

```linear
fn update(!state: State, config: Config, @event: Event) -> State {
  apply(!state, config, @event: event)
}

cache.insert!(key: "latest", @value: event)
```

This is documentation and sugar for value threading. The checker/lowerer must
eventually verify that the body agrees with the declared flow.

## Open Syntax

Still unsettled:

- collection builder notation;
- iterator/pipeline notation;
- exact trait semantics beyond parsing;
- how much generic syntax will lower to monomorphized core;
- whether anonymous enum syntax remains in the final surface.
