-- Keep the chat and thread partition while removing obsolete sender metadata.
UPDATE sessions
   SET context = jsonb_set(context, '{chat}', (context->'chat') - ARRAY['sender_id_hash', 'owner'])
 WHERE (context->'chat') ?| ARRAY['sender_id_hash', 'owner'];

UPDATE observations
   SET context = jsonb_set(context, '{chat}', (context->'chat') - ARRAY['sender_id_hash', 'owner'])
 WHERE (context->'chat') ?| ARRAY['sender_id_hash', 'owner'];
