-- store content encryption keys in the record
alter table records
  add column cek text;
