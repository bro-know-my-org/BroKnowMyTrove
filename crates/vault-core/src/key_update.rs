use crate::{
    Error, Result, Vault, codec,
    crypto::{Password, RecoveryKey},
    keyfile::KeyFile,
    operation::{Operation, Point},
    store::sync_directory,
};
use std::io::Write;

impl Vault {
    /// Rewraps the same master key, keeping the KDF parameters and recovery slot.
    /// Old credentials with old key-file copies still decrypt the same master key.
    pub fn change_password(&mut self, password: &Password, operation: &Operation) -> Result<()> {
        let mut header = self.key_header(operation)?;
        header.rewrap_password(&self.master, password)?;
        self.publish_key_header(header, operation)
    }

    /// Creates/replaces the recovery slot without changing the password slot.
    /// On an uncertain publication, reopen using the password and rotate again.
    pub fn rotate_recovery_key(&mut self, operation: &Operation) -> Result<RecoveryKey> {
        let mut header = self.key_header(operation)?;
        let key = RecoveryKey::generate()?;
        header.rewrap_recovery(&self.master, Some(&key))?;
        self.publish_key_header(header, operation)?;
        Ok(key)
    }

    /// Removes recovery from this copy only. Old copies retain their recovery slot.
    pub fn remove_recovery_key(&mut self, operation: &Operation) -> Result<()> {
        let mut header = self.key_header(operation)?;
        header.rewrap_recovery(&self.master, None)?;
        self.publish_key_header(header, operation)
    }

    fn key_header(&mut self, operation: &Operation) -> Result<KeyFile> {
        operation.check()?;
        self.check_writable()?;
        self.check_key_source()?;
        codec::decode(&self.key_bytes, codec::KEY_LIMIT)
    }

    fn check_key_source(&mut self) -> Result<()> {
        if *codec::read_limited(&self.store.root.join("keys.json"), codec::KEY_LIMIT)?
            != *self.key_bytes
        {
            self.writable = false;
            return Err(Error::SourceChanged);
        }
        Ok(())
    }

    fn publish_key_header(&mut self, header: KeyFile, operation: &Operation) -> Result<()> {
        operation.checkpoint(Point::KeyUpdateDerived)?;
        operation.check()?;
        let bytes = codec::encode(&header, codec::KEY_LIMIT)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".tmp-key-")
            .tempfile_in(&self.store.root)?;
        operation.writer(&mut temporary).write_all(&bytes)?;
        operation.checkpoint(Point::KeyUpdateWritten)?;
        temporary.as_file().sync_all()?;
        operation.checkpoint(Point::KeyUpdateSynced)?;
        self.check_key_source()?;
        operation.check()?;
        // Same-directory atomic replacement; no remove-then-rename gap.
        temporary
            .persist(self.store.root.join("keys.json"))
            .map_err(|error| Error::from(error.error))?;
        self.key_bytes = bytes;
        // Publication ends cancellation. Always attempt directory sync, even if
        // the post-publication fault reports an error.
        let published = operation.checkpoint(Point::KeyUpdatePublished);
        let synced = sync_directory(&self.store.root);
        let finished = operation.checkpoint(Point::KeyUpdateDirectorySynced);
        if published.and(synced).and(finished).is_err() {
            self.writable = false;
            return Err(Error::CommitUncertain);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
