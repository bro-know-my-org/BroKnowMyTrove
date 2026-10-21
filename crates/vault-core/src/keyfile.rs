use serde::{Deserialize, Serialize};

use crate::{
    Error, Id, Result,
    crypto::{self, KdfParams, Password, RecoveryKey, Secret},
};

const MAGIC: &str = "BKMT-1";

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
    pub authentication: Vec<u8>,
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
            magic: MAGIC.into(),
            vault_id: Id::random()?,
            kdf: options.kdf,
            salt: crypto::random_array()?,
            password: Vec::new(),
            recovery: None,
            authentication: Vec::new(),
        };
        let wrapping = header.kdf.derive(password, &header.salt)?;
        header.password = crypto::seal(&wrapping, &header.password_aad(), master.0.as_slice())?;
        header.recovery = recovery
            .as_ref()
            .map(|key| crypto::seal(key.secret(), &header.recovery_aad(), master.0.as_slice()))
            .transpose()?;
        header.authenticate(&master)?;
        Ok((header, master, recovery))
    }

    pub fn unlock(&self, credential: Credential<'_>) -> Result<Secret> {
        if self.magic != MAGIC
            || self.password.len() != 72
            || self.recovery.as_ref().is_some_and(|slot| slot.len() != 72)
            || self.authentication.len() != 40
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
        let master = Secret::from_bytes(&plaintext)?;
        crypto::unseal(
            &master.derive(b"BKMKEYS1")?,
            &self.authentication_aad(),
            &self.authentication,
        )?;
        Ok(master)
    }

    fn password_aad(&self) -> Vec<u8> {
        let mut aad = b"BKMTKEY1".to_vec();
        aad.extend_from_slice(&self.vault_id.0);
        aad.extend_from_slice(&self.kdf.operations.to_le_bytes());
        aad.extend_from_slice(&self.kdf.memory_bytes.to_le_bytes());
        aad.extend_from_slice(&self.salt);
        aad
    }

    pub(crate) fn rewrap_password(&mut self, master: &Secret, password: &Password) -> Result<()> {
        // Keep the production floor even when created with cheaper core API settings.
        let minimum = KdfParams::default();
        self.kdf.operations = self.kdf.operations.max(minimum.operations);
        self.kdf.memory_bytes = self.kdf.memory_bytes.max(minimum.memory_bytes);
        self.salt = crypto::random_array()?;
        let wrapping = self.kdf.derive(password, &self.salt)?;
        self.password = crypto::seal(&wrapping, &self.password_aad(), master.0.as_slice())?;
        self.authenticate(master)?;
        Ok(())
    }

    pub(crate) fn rewrap_recovery(
        &mut self,
        master: &Secret,
        key: Option<&RecoveryKey>,
    ) -> Result<()> {
        self.recovery = key
            .map(|key| crypto::seal(key.secret(), &self.recovery_aad(), master.0.as_slice()))
            .transpose()?;
        self.authenticate(master)?;
        Ok(())
    }

    // An empty AEAD message authenticates both credential slots and all parameters.
    // A separate master-derived key lets password changes preserve recovery access.
    fn authenticate(&mut self, master: &Secret) -> Result<()> {
        self.authentication = crypto::seal(
            &master.derive(b"BKMKEYS1")?,
            &self.authentication_aad(),
            &[],
        )?;
        Ok(())
    }

    fn authentication_aad(&self) -> Vec<u8> {
        let mut aad = b"BKMTKEYS1".to_vec();
        aad.extend_from_slice(self.magic.as_bytes());
        aad.extend_from_slice(&self.password_aad());
        aad.extend_from_slice(&self.password);
        aad.push(u8::from(self.recovery.is_some()));
        if let Some(recovery) = &self.recovery {
            aad.extend_from_slice(recovery);
        }
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
    fn frozen_format_opens_independent_libsodium_reference_vector() {
        #[derive(Deserialize)]
        struct Vector {
            password: String,
            recovery: String,
            master_hex: String,
            keyfile: KeyFile,
        }
        let vector: Vector =
            serde_json::from_str(include_str!("../tests/vectors/keyfile.json")).unwrap();
        let password = Password::new(vector.password).unwrap();
        let recovery = RecoveryKey::decode(&vector.recovery).unwrap();
        let expected = hex::decode(vector.master_hex).unwrap();
        for credential in [
            Credential::Password(&password),
            Credential::Recovery(&recovery),
        ] {
            assert_eq!(
                vector.keyfile.unlock(credential).unwrap().0.as_slice(),
                expected
            );
        }
        // Re-encoding in Rust must preserve the independent reference semantics.
        let bytes = crate::codec::encode(&vector.keyfile, crate::codec::KEY_LIMIT).unwrap();
        let header: KeyFile = crate::codec::decode(&bytes, crate::codec::KEY_LIMIT).unwrap();
        assert_eq!(
            header
                .unlock(Credential::Recovery(&recovery))
                .unwrap()
                .0
                .as_slice(),
            expected
        );
    }

    #[test]
    fn recovery_rejects_tampered_kdf_salt_password_and_recovery_presence() {
        let password = Password::new("认证整个密钥入口".into()).unwrap();
        let (header, _, recovery) = KeyFile::create(
            &password,
            CreateOptions {
                kdf: KdfParams {
                    operations: 2,
                    memory_bytes: 16 * 1024 * 1024,
                },
                recovery_key: true,
            },
        )
        .unwrap();
        let recovery = recovery.unwrap();
        let encoded = crate::codec::encode(&header, crate::codec::KEY_LIMIT).unwrap();
        for field in [
            "vault_id",
            "operations",
            "memory",
            "salt",
            "password",
            "recovery",
            "recovery_slot",
            "authentication",
        ] {
            let mut changed: KeyFile =
                crate::codec::decode(&encoded, crate::codec::KEY_LIMIT).unwrap();
            match field {
                "vault_id" => changed.vault_id.0[0] ^= 1,
                "operations" => changed.kdf.operations = 1,
                "memory" => changed.kdf.memory_bytes = 8 * 1024 * 1024,
                "salt" => changed.salt[0] ^= 1,
                "password" => changed.password[24] ^= 1,
                "recovery" => changed.recovery = None,
                "recovery_slot" => changed.recovery.as_mut().unwrap()[24] ^= 1,
                "authentication" => changed.authentication[24] ^= 1,
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    changed.unlock(Credential::Recovery(&recovery)),
                    Err(Error::Authentication)
                ),
                "recovery accepted tampered {field}"
            );
            assert!(
                matches!(
                    changed.unlock(Credential::Password(&password)),
                    Err(Error::Authentication)
                ),
                "password accepted tampered {field}"
            );
        }
    }

    #[test]
    fn stable_header_requires_authentication_and_rejects_experimental_magic() {
        let password = Password::new("首版格式".into()).unwrap();
        let (mut header, _, _) = KeyFile::create(
            &password,
            CreateOptions {
                kdf: KdfParams {
                    operations: 1,
                    memory_bytes: 8 * 1024 * 1024,
                },
                recovery_key: false,
            },
        )
        .unwrap();
        assert_eq!(header.magic, "BKMT-1");
        let mut value = serde_json::to_value(&header).unwrap();
        value.as_object_mut().unwrap().remove("authentication");
        assert!(matches!(
            crate::codec::decode::<KeyFile>(
                &serde_json::to_vec(&value).unwrap(),
                crate::codec::KEY_LIMIT
            ),
            Err(Error::InvalidFormat)
        ));
        header.authentication.pop();
        assert!(matches!(
            header.unlock(Credential::Password(&password)),
            Err(Error::InvalidFormat)
        ));
        header.authentication.push(0);
        header.magic = "BKMT-EXPERIMENT-1".into();
        assert!(matches!(
            header.unlock(Credential::Password(&password)),
            Err(Error::InvalidFormat)
        ));
    }

    #[test]
    fn rewrap_preserves_stronger_stored_kdf_components() {
        let password = Password::new("测试重新包装".into()).unwrap();
        let (mut header, master, _) = KeyFile::create(
            &password,
            CreateOptions {
                kdf: KdfParams {
                    operations: 1,
                    memory_bytes: 8 * 1024 * 1024,
                },
                recovery_key: false,
            },
        )
        .unwrap();
        header.kdf.operations = KdfParams::default().operations + 1;
        header.rewrap_password(&master, &password).unwrap();
        assert_eq!(header.kdf.operations, KdfParams::default().operations + 1);
        assert_eq!(header.kdf.memory_bytes, KdfParams::default().memory_bytes);
        assert_eq!(
            *header.unlock(Credential::Password(&password)).unwrap().0,
            *master.0
        );
    }

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
