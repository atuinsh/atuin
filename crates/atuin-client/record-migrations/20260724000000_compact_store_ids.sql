-- Store record and host uuids as 16-byte blobs rather than 36-char hyphenated
-- text; on a large store the two columns and their indexes are tens of MB.
-- coalesce guards anything unexpectedly non-uuid, leaving it untouched.
-- unhex() needs the bundled SQLite >= 3.41.
update store set
    id = coalesce(unhex(replace(id, '-', '')), id),
    host = coalesce(unhex(replace(host, '-', '')), host)
where typeof(id) = 'text' or typeof(host) = 'text';
