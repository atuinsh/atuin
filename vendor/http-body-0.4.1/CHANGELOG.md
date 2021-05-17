# Unreleased

Nothing.

# 0.4.1 (March 18, 2021)

- Add combinators to `Body`:
  - `map_data`: Change the `Data` chunks produced by the body.
  - `map_err`: Change the `Error`s produced by the body.
  - `boxed`: Convert the `Body` into a boxed trait object.
- Add `Empty`.

# 0.4.0 (December 23, 2020)

- Update `bytes` to v1.0.

# 0.3.1 (December 13, 2019)

- Implement `Body` for `http::Request<impl Body>` and `http::Response<impl Body>`.

# 0.3.0 (December 4, 2019)

- Rename `next` combinator to `data`.

# 0.2.0 (December 3, 2019)

- Update `http` to v0.2.
- Update `bytes` to v0.5.

# 0.2.0-alpha.3 (October 1, 2019)

- Fix `Body` to be object-safe.

# 0.2.0-alpha.2 (October 1, 2019)

- Add `next` and `trailers` combinator methods.

# 0.2.0-alpha.1 (August 20, 2019)

- Update to use `Pin` in `poll_data` and `poll_trailers`.

# 0.1.0 (May 7, 2019)

- Initial release
