Add a new subcommand in the CLI called migrate-old-code

This would be used as we change the language to help migrate old code into new code. So we'll add transformation rules as in there as we go. if we currently have any transformations like that then move them into this new tool. The other functions should only have logic that handles the latest syntax and so if there's any code using old syntax, it would need to use this subcommand first before that code could be used in any other way. 

This will be used for a variety of things, including the upcoming annotation syntax and possible other syntactic changes. 