-- Before this commit was added, we had a bug where we didn't delete unused
-- records when users were deleted. Oops.
delete from store where user_id not in (select id from users);
