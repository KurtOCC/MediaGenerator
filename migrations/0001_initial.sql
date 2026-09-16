-- Initial schema for Mediagenerator.
--
-- Timestamps are timestamptz throughout: the application runs in Azure and is
-- used from Norway, so anything without a zone would be ambiguous twice a year.
-- gen_random_uuid() is built into PostgreSQL 13 and later, so no extension is
-- needed.

create table users (
    id            uuid primary key     default gen_random_uuid(),
    -- Entra ID object id. Stable across applications in the tenant, unlike
    -- `sub`, which is pairwise per app registration.
    entra_oid     text        not null unique,
    email         text,
    display_name  text        not null,
    created_at    timestamptz not null default now(),
    last_login_at timestamptz not null default now()
);

create type media_type as enum ('image', 'audio', 'video');

create table jobs (
    id              uuid primary key     default gen_random_uuid(),
    user_id         uuid        not null references users (id) on delete cascade,
    media_type      media_type  not null,
    prompt          text        not null,
    -- Provider parameters (size, quality, voice, …) as submitted.
    parameters      jsonb       not null default '{}'::jsonb,
    -- queued | running | succeeded | failed | cancelled.
    status          text        not null,
    -- Identifier at the provider, for asynchronous video jobs.
    provider_job_id text,
    error_code      text,
    error_message   text,
    created_at      timestamptz not null default now(),
    started_at      timestamptz,
    completed_at    timestamptz,

    constraint jobs_status_valid
        check (status in ('queued', 'running', 'succeeded', 'failed', 'cancelled')),
    -- A failed job must say why, and a job that has not failed must not.
    constraint jobs_error_matches_status
        check ((status = 'failed') = (error_code is not null))
);

-- The history page reads one user's jobs, newest first.
create index jobs_user_created_idx on jobs (user_id, created_at desc);

-- The worker and the "do you have anything running?" query both look only at
-- unfinished jobs, which are a small minority of the table.
create index jobs_active_idx on jobs (created_at)
    where status in ('queued', 'running');

create table assets (
    id           uuid primary key     default gen_random_uuid(),
    job_id       uuid        not null references jobs (id) on delete cascade,
    -- Path inside the private blob container. Never a full URL: the account
    -- and container come from configuration, and the URL handed to a browser
    -- is a time-limited SAS generated per request.
    blob_path    text        not null,
    content_type text        not null,
    size_bytes   bigint      not null,
    duration_ms  integer,
    width        integer,
    height       integer,
    created_at   timestamptz not null default now(),

    constraint assets_size_non_negative check (size_bytes >= 0)
);

create index assets_job_idx on assets (job_id);

create table audit_log (
    id         bigserial primary key,
    -- Kept when the user is deleted: an audit trail that disappears with the
    -- account it describes is not an audit trail.
    user_id    uuid references users (id) on delete set null,
    action     text        not null,
    entity     text        not null,
    entity_id  text,
    ip         inet,
    user_agent text,
    created_at timestamptz not null default now()
);

create index audit_log_user_created_idx on audit_log (user_id, created_at desc);
create index audit_log_created_idx on audit_log (created_at);
