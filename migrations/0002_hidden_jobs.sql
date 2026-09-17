-- Let a user keep a generation out of the shared archive without deleting it.
--
-- Hidden is not private: the owner still sees it in their own history and in
-- the "egne" view of the archive. It only means "do not show this to
-- colleagues". Deleting is the separate, irreversible option.

alter table jobs add column hidden boolean not null default false;

-- The shared archive reads only what is visible, so the partial index covers
-- exactly the rows that listing touches.
create index jobs_visible_idx on jobs (created_at desc)
    where status = 'succeeded' and not hidden;
