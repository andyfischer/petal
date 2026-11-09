Do an investigation on the possibility of adding the at sign to mean annotations and possibly have it mean the same exact thing that it means in Python. if I understand right, it's just syntactic sugar and it means to run this declaration or expression through this function call and use the result. this could be a good fit for our static annotations like 'const' and 'tunable'. 

Do an investigation on this, if it would work well, make sure it doesn't have conflicts with existing language, check the Python semantics and figure out if we would be able to match Python's semantics exactly or if there would be a difference and see if there's any other issues to use this.

I think it would especially be used on function declarations and on LED bindings and potentially on expressions too. I'm not totally sure about that. It depends if there would be any issues. 

Note that the language had an old form of using the at symbol as the rebinding operator. I think we can safely deprecate and delete that syntax form. So if it's still there, let's just get rid of it. 

## Status

Investigated 9 November 2026. The proposal is
[docs/proposals/annotation-syntax.md](../proposals/annotation-syntax.md).
In short: `@` annotations on top-level `let` declarations, lowered by the
parser to the existing `const(…)`/`tunable(…)`/`rigid(…)` calls, with the
value inserted as the first argument (Python's curried order does not fit
Petal's builtins). No run-time decorators on `fn` for now: they conflict with
hoisting, direct self-recursion, overloading and method pinning. The old `@`
rebind operator still exists (one `.ptl` file uses it); its full inventory is
in the proposal, and it is being removed separately. Open questions for the
owner are at the end of the proposal.
