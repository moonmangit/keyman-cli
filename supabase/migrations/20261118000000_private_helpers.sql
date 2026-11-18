-- Keep the RLS helper functions and the trigger function out of the REST API.
-- On a hosted project the default privileges of schema public give EXECUTE on each new
-- function to authenticated, so every function in public is callable at /rest/v1/rpc/<name>.
-- Only create_vault, accept_invite, rotate_vault and my_profile are API calls.
-- Policies and the trigger refer to a function by OID, so they keep working after the move.
create schema if not exists private;
revoke all on schema private from public, anon;
grant usage on schema private to authenticated;

alter function public.my_vault_ids() set schema private;
alter function public.is_vault_owner(uuid) set schema private;
alter function public.is_group_owner(uuid) set schema private;
alter function public.my_verified_email() set schema private;
alter function public.vault_generation(uuid) set schema private;
alter function public.projects_guard() set schema private;

-- A trigger function runs without an EXECUTE grant to the caller; nobody calls it directly.
revoke execute on function private.projects_guard() from public, anon, authenticated;
