
Contributing to sodiumoxide
---
* [Feature Requests](#feature-requests)
* [Bug Reports](#bug-reports)
* [Pull Requests](#pull-requests)
* [Writing Documentation](#writing-documentation)
* [Issue Triage](#issue-triage)
* [Out-of-tree Contributions](#out-of-tree-contributions)
* [Helpful Links](#helpful-links)

For any questions, please make a post on [users.rust-lang.org][u-r-l-o], post
on [sodiumoxide-rs mailing list] or join our [gitter] channel.

> All contributors need to follow our [Code of Conduct].

[Code of Conduct]: CODE_OF_CONDUCT.md

# Feature Requests
[Feature Requests]: #feature-requests

The `sodiumoxide` crate is still in flux. All features desired may not be present. As
such you are welcome to request for new features. 

If you have the chance, please [search existing issues], as there is a chance
that someone has already requested your feature.

File your feature request with a descriptive title, as this helps others find
your request.

You can request your feature by following [this link][Feature Request Link] and
filling it in. 

> We welcome pull requests for your own feature requests, provided they have
been discussed.

# Bug Reports
[Bug Reports]: #bug-reports

While no one likes bugs, they are an unfortunate reality in software. Remember
we can't fix bugs we don't know about, so don't be shy about reporting.

If you have the chance, please [search existing issues], as there is a chance
that someone has already reported your error. This isn't strictly needed, as
sometimes you might not what exactly you are looking for.

File your issue with a descriptive title, as this helps others find your issue.

Sometimes a backtrace may be needed. In that case, set `RUST_BACKTRACE`
environment variable to `1`. For example:

```bash
$ RUST_BACKTRACE=1 cargo build
```

> We welcome pull requests for your own bug reports, provided they have been
discussed.


# Pull Requests
[Pull Requests]: #pull-requests

Pull requests(PRs) are the primary mechanism we use to change sodiumoxide. GitHub itself
has some [great documentation] on using the Pull Request feature. We use the
"fork and pull" model described [here][fnp], where contributors push changes to
their personal fork and create pull requests to bring those changes into the
source repository.

Ensure that your changes conform to how the rest of the crate is written and follows a 
similar API, patterns, namings & other conventions. Also ensure that your code is formatted
using the latest version of [rustfmt]. (Ensure you update your nightly before running rustfmt)

Your changes should contain real vectors from the underlying library along with sanity tests
in Rust. 

Please open PRs against `master` branch.

If the pull request is still a work in progress, prepend`[WIP] ` in your 
title. 

When you feel that the PR is ready, please ping one of the maintainers so
they can review your changes.

[great documentation]: https://help.github.com/articles/about-pull-requests/
[fnp]: https://help.github.com/articles/about-collaborative-development-models/
[rustfmt]: https://github.com/rust-lang/rustfmt

# Writing Documentation
[Writing Documentation]: #writing-documentation

Documentation is an important part of sodiumoxide. Lackluster or incorrect
documentation can cause headaches for the users of `sodiumoxide`. Therefore,
improvements to documentation are always welcome.


# Issue Triage
[Issue Triage]: #issue-triage

Sometimes, an issue might stay open even after the relevant bug has been fixed.
Other times, the bug report may become invalid. Or we may just forget about the
bug.

You can help to go through old bug reports and check if they are still valid.
You can follow [this link][lrus] to look for issues like this.

[lrus]: https://github.com/sodiumoxide/sodiumoxide/issues?q=is%3Aissue+is%3Aopen+sort%3Aupdated-asc

# Out-of-tree Contributions
[Out-of-tree Contributions]: #out-of-tree-contributions

You can contribute to sodiumoxide in other ways:

* Answer questions on [users.rust-lang.org][u-r-l-o] or
[gitter] channel.
* Find the [crates depending on `sodiumoxide`][dependent] and sending PRs to them,
helping them keep their version of `sodiumoxide` up-to-date.

[dependent]: https://crates.io/crates/sodiumoxide/reverse_dependencies



[u-r-l-o]: https://users.rust-lang.org
[gitter]: https://gitter.im/sodiumoxide-rs/Lobby
[search existing issues]: https://github.com/sodiumoxide/sodiumoxide/search?q=&type=Issues&utf8=%E2%9C%93
