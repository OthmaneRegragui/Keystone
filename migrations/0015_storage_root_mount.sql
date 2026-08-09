-- The "storagedata" volume is now mounted at /mnt/keystone/data inside the
-- container instead of /data/storage. Rewrite absolute storage paths so
-- existing buckets keep working after the mount change.
-- storage_objects.storage_path is relative (shard path under a bucket root)
-- and is intentionally left untouched.
UPDATE storage_paths
   SET path = '/mnt/keystone/data' || substr(path, length('/data/storage') + 1)
 WHERE path LIKE '/data/storage%';

UPDATE buckets
   SET path = '/mnt/keystone/data' || substr(path, length('/data/storage') + 1)
 WHERE path LIKE '/data/storage%';
