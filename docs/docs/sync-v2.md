# Sync v2

Just installed Atuin? Don't worry about this page, everything should be set up
for you.

If you've been using Atuin for a while, you might not be using the best sync. A
long time ago, we shipped sync v1. It was "good enough", but never intended for
the wide use it ended up getting.

After evaluating issues and feedback from users, we developed sync v2. It
includes a whole bunch of changes that I'll save writing about for a future
blog post or deep dive, but suffice to say it's

1. Faster
2. More reliable
3. Uses less bandwidth
4. Supports many more features
5. Recovers from issues more effectively

There's really no reason to not use it.

You can opt in with the following configuration

```toml
[sync]
records = true
```
