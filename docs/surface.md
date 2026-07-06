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
struct CopyStore { users: List<u32> }: Dup + Zap

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

`Dup` and `Zap` are written like trait bounds on type declarations:

```linear
struct CopyStore { users: List<u32> }: Dup + Zap
enum DropEvent { item(u32) }: Zap
```

They are trait-like at the surface, but currently compiler-recognized
capabilities, not ordinary user traits. They change the linearity rules:
`Dup` permits explicit duplication and `Zap` permits explicit dropping.

Composite declarations may only declare capabilities they already have
structurally. This is rejected because `MutList<u32>` is linear:

```linear
struct Bad { work: MutList<u32> }: Dup
```

Capability clauses currently lower only on nominal `struct` and `enum`
declarations. A `type` alias with a capability clause is parsed but rejected by
lowering, because that would no longer be a plain alias:

```linear
type Users = HashMap<UserId, User>: Dup // rejected for now
```

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
insert(users, key: id, take value: user)
```

Method syntax is sugar for calling a function with `self` as the first
argument.

```linear
users.insert mut(key: id, take value: user)
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

Declared `Dup`/`Zap` capabilities on nominal structs and enums lower into core
`DeclaredCapabilities`, then the type store verifies they do not exceed the
type's structural capabilities. Unknown capability names are rejected.
Capability clauses on `type` aliases are rejected until aliases can either
remain pure aliases or become explicit newtypes.

The signature lowering pass builds on this and registers:

- global declarations;
- free function parameter and output types;
- inherent impl methods, with `self` expanded to the impl target type;
- trait impl methods as named functions, without lowering trait semantics yet.

Surface function output means the visible suffix after threaded values. Core
function outputs are formed by first appending every non-`take` parameter type,
in declaration order, then appending the visible surface output when it is not
unit.

```linear
fn step(mut state: State, config: Config, take event: Event) -> Decision
```

lowers to a core function returning `(State, Config, Decision)`. `State` is the
changed threaded value, `Config` is the unchanged threaded value, and `Decision`
is the visible result.

The resulting `CoreProgram` is a name/type skeleton. Function bodies are kept as
frontend AST for the later body-lowering pass, so the skeleton is not expected
to pass core checking yet.

The first body-lowering pass supports a deliberately small executable subset:

- function parameters and local names;
- `let name = expr` and typed scalar lets;
- integer literals;
- direct calls to known functions, including hidden threaded return values;
- references to declared globals;
- product constructors such as `Pair { left: x, right: y }`;
- enum constructors such as `Maybe.some(x)` and `Maybe.none`;
- `match` over enums, including tuple and record payload patterns;
- finite `+`, `-`, `*`, `==`, `<`, and `>` over integer-like finite types.

Body lowering automatically returns every non-`take` parameter first. The final
body expression supplies only the visible surface result. Finite infix
operators lower as ordinary observer-style primitive calls: they consume their
operands and return those operands unchanged, followed by the visible result.
So this:

```linear
fn below_ten(x: u32) -> Bool {
  x < 10
}
```

returns `(x, Bool)` at core level without inserting a frontend `dup`. A function
that literally returns the same visible value that is also implicitly returned
still needs a real duplication operation; the body lowerer does not invent one.
`mut` parameters are not auto-duplicated either. If a body consumes one without
rebinding it through a call result, the core checker rejects the function.

Calls consume all arguments at core level. For any callee argument declared
without `take`, the surface argument must be a name so the callee's hidden
returned value can be rebound to that name. For `take` arguments, ordinary
expressions are allowed.

It lowers free functions and impl methods into checked core functions. Ordinary
field access, non-constructor methods at call sites, `if`, string literals, and
collection operations are still intentionally rejected until their lowering
rules are added.

## Value Flow Markers

At function definitions and call sites:

- no marker: the argument is returned unchanged;
- `mut`: the argument is returned changed;
- `take`: the argument is consumed and not returned.

Markers can appear before parameters and before call arguments. Call-site
markers are optional when the callee signature already determines the flow, but
they are useful documentation and mismatched explicit markers are rejected. For
method syntax, a marker between the method name and the argument list marks the
receiver:

```linear
fn update(mut state: State, config: Config, take event: Event) -> State {
  apply(mut state, config, take event: event)
}

cache.insert mut(key: "latest", take value: event)
```

This is documentation and sugar for value threading. The checker/lowerer must
verify that the body agrees with the declared flow; current lowering makes the
threaded return convention explicit and leaves detailed linear-use validation to
the core checker.

## Open Syntax

Still unsettled:

- collection builder notation;
- iterator/pipeline notation;
- exact trait semantics beyond parsing;
- how much generic syntax will lower to monomorphized core;
- whether anonymous enum syntax remains in the final surface.
