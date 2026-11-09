Do an investigation on the possibility of adding the at sign to mean annotations and possibly have it mean the same exact thing that it means in Python. if I understand right, it's just syntactic sugar and it means to run this declaration or expression through this function call and use the result. this could be a good fit for our static annotations like 'const' and 'tunable'. 

Do an investigation on this, if it would work well, make sure it doesn't have conflicts with existing language, check the Python semantics and figure out if we would be able to match Python's semantics exactly or if there would be a difference and see if there's any other issues to use this.

I think it would especially be used on function declarations and on LED bindings and potentially on expressions too. I'm not totally sure about that. It depends if there would be any issues. 

Note that the language had an old form of using the at symbol as the rebinding operator. I think we can safely deprecate and delete that syntax form. So if it's still there, let's just get rid of it. 