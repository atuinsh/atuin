alter table history add column author text;
alter table history add column intent text;

update history
set author = case
    when instr(hostname, ':') > 0 then substr(hostname, instr(hostname, ':') + 1)
    else hostname
end
where author is null or trim(author) = '';
