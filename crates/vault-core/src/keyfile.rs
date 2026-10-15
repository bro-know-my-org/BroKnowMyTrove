use serde::{Deserialize, Serialize};

use crate::{
    Error, Id, Result,
    crypto::{self, KdfParams, Password, RecoveryKey, Secret},
};

pub enum Credential<'a> {
    Password(&'a Password),
    Recovery(&'a RecoveryKey),
}

#[derive(Default)]
pub struct CreateOptions {
    pub kdf: KdfParams,
    pub recovery_key: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyFile {
    pub magic: String,
    pub vault_id: Id,
    pub kdf: KdfParams,
    pub salt: [u8; 16],
    pub password: Vec<u8>,
    pub recovery: Option<Vec<u8>>,
}

impl KeyFile {
    pub fn create(
        password: &Password,
        options: CreateOptions,
    ) -> Result<(Self, Secret, Option<RecoveryKey>)> {
        options.kdf.validate()?;
        let master = Secret::generate()?;
        let recovery = options
            .recovery_key
            .then(RecoveryKey::generate)
            .transpose()?;
        let mut header = Self {
            magic: "BKMT-EXPERIMENT-1".into(),
            vault_id: Id::random()?,
            kdf: options.kdf,
            salt: crypto::random_array()?,
            password: Vec::new(),
            recovery: None,
        };
        let wrapping = header.kdf.derive(password, &header.salt)?;
        header.password = crypto::seal(&wrapping, &header.password_aad(), master.0.as_slice())?;
        header.recovery = recovery
            .as_ref()
            .map(|key| crypto::seal(key.secret(), &header.recovery_aad(), master.0.as_slice()))
            .transpose()?;
        Ok((header, master, recovery))
    }

    pub fn unlock(&self, credential: Credential<'_>) -> Result<Secret> {
        if self.magic != "BKMT-EXPERIMENT-1"
            || self.password.len() != 72
            || self.recovery.as_ref().is_some_and(|slot| slot.len() != 72)
        {
            return Err(Error::InvalidFormat);
        }
        self.kdf.validate()?;
        let plaintext = match credential {
            Credential::Password(password) => crypto::unseal(
                &self.kdf.derive(password, &self.salt)?,
                &self.password_aad(),
                &self.password,
            )?,
            Credential::Recovery(key) => crypto::unseal(
                key.secret(),
                &self.recovery_aad(),
                self.recovery.as_ref().ok_or(Error::Authentication)?,
            )?,
        };
        Secret::from_bytes(&plaintext)
    }

    fn password_aad(&self) -> Vec<u8> {
        let mut aad = b"BKMTKEY1".to_vec();
        aad.extend_from_slice(&self.vault_id.0);
        aad.extend_from_slice(&self.kdf.operations.to_le_bytes());
        aad.extend_from_slice(&self.kdf.memory_bytes.to_le_bytes());
        aad.extend_from_slice(&self.salt);
        aad
    }

    fn recovery_aad(&self) -> Vec<u8> {
        let mut aad = b"BKMTREC1".to_vec();
        aad.extend_from_slice(&self.vault_id.0);
        aad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip_recovery_wrong_password_and_resource_rejection() {
        let password = Password::new("第一句中文e\u{301}口令".into()).unwrap();
        let options = CreateOptions {
            kdf: KdfParams {
                operations: 1,
                memory_bytes: 8 * 1024 * 1024,
            },
            recovery_key: true,
        };
        let (header, master, recovery) = KeyFile::create(&password, options).unwrap();
        let encoded = crate::codec::encode(&header, crate::codec::KEY_LIMIT).unwrap();
        let mut header: KeyFile = crate::codec::decode(&encoded, crate::codec::KEY_LIMIT).unwrap();
        assert_eq!(
            *header.unlock(Credential::Password(&password)).unwrap().0,
            *master.0
        );
        assert_eq!(
            *header
                .unlock(Credential::Recovery(&recovery.unwrap()))
                .unwrap()
                .0,
            *master.0
        );
        assert!(
            header
                .unlock(Credential::Password(&Password::new("错误".into()).unwrap()))
                .is_err()
        );
        header.kdf.memory_bytes = u64::MAX;
        assert!(matches!(
            header.unlock(Credential::Password(&password)),
            Err(Error::ResourceLimit)
        ));
        header.magic = "BKMT-EXPERIMENT-2".into();
        assert!(matches!(
            header.unlock(Credential::Password(&password)),
            Err(Error::InvalidFormat)
        ));
    }

    #[test]
    fn metadata_authentication_binds_vault_purpose_and_physical_id() {
        let key = Secret::generate().unwrap();
        let aad = crate::codec::aad(1, Id([1; 16]), Id([2; 16]));
        let bytes = crate::codec::seal_json(&key, &aad, &vec![1u64, 2], 4096).unwrap();
        let opened: Vec<u64> = crate::codec::unseal_json(&key, &aad, &bytes, 4096).unwrap();
        assert_eq!(opened, vec![1, 2]);
        for changed in [
            crate::codec::aad(2, Id([1; 16]), Id([2; 16])),
            crate::codec::aad(1, Id([9; 16]), Id([2; 16])),
            crate::codec::aad(1, Id([1; 16]), Id([9; 16])),
        ] {
            assert!(crate::codec::unseal_json::<Vec<u64>>(&key, &changed, &bytes, 4096).is_err());
        }
        assert!(crate::codec::decode::<KeyFile>(&vec![0; 16385], 16384).is_err());
    }
}
