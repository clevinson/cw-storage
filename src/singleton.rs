use named_type::NamedType;
use serde::{de::DeserializeOwned, ser::Serialize};
use snafu::{OptionExt, ResultExt};
use std::marker::PhantomData;

use cosmwasm::errors::{ContractErr, ParseErr, Result, SerializeErr};
use cosmwasm::serde::{from_slice, to_vec};
use cosmwasm::traits::{ReadonlyStorage, Storage};

use crate::prefix::key_prefix;

// singleton is a helper function for less verbose usage
pub fn singleton<'a, S: Storage, T>(storage: &'a mut S, key: &[u8]) -> Singleton<'a, S, T>
where
    T: Serialize + DeserializeOwned + NamedType,
{
    Singleton::new(storage, key)
}

// singleton_read is a helper function for less verbose usage
pub fn singleton_read<'a, S: ReadonlyStorage, T>(
    storage: &'a S,
    key: &[u8],
) -> ReadonlySingleton<'a, S, T>
where
    T: Serialize + DeserializeOwned + NamedType,
{
    ReadonlySingleton::new(storage, key)
}

/// Singleton effectively combines PrefixedStorage with TypedStorage to
/// work on one single value. It performs the key_prefix transformation
/// on the given name to ensure no collisions, and then provides the standard
/// TypedStorage accessors, without requiring a key (which is defined in the constructor)
pub struct Singleton<'a, S: Storage, T>
where
    T: Serialize + DeserializeOwned + NamedType,
{
    storage: &'a mut S,
    key: Vec<u8>,
    // see https://doc.rust-lang.org/std/marker/struct.PhantomData.html#unused-type-parameters for why this is needed
    data: PhantomData<&'a T>,
}

impl<'a, S: Storage, T> Singleton<'a, S, T>
where
    T: Serialize + DeserializeOwned + NamedType,
{
    pub fn new(storage: &'a mut S, key: &[u8]) -> Self {
        Singleton {
            storage,
            key: key_prefix(key),
            data: PhantomData,
        }
    }

    /// save will serialize the model and store, returns an error on serialization issues
    pub fn save(&mut self, data: &T) -> Result<()> {
        let bz = to_vec(data).context(SerializeErr {
            kind: T::short_type_name(),
        })?;
        self.storage.set(&self.key, &bz);
        Ok(())
    }

    /// load will return an error if no data is set at the given key, or on parse error
    pub fn load(&self) -> Result<T> {
        // TODO: replace with NotFound with cosmwasm 0.6.1
        self.may_load()?.context(ContractErr {
            msg: "uninitialized data",
        })
    }

    /// may_load will parse the data stored at the key if present, returns Ok(None) if no data there.
    /// returns an error on issues parsing
    pub fn may_load(&self) -> Result<Option<T>> {
        match self.storage.get(&self.key) {
            Some(d) => from_slice(&d).context(ParseErr {
                kind: T::short_type_name(),
            }),
            None => Ok(None),
        }
    }

    /// update will load the data, perform the specified action, and store the result
    /// in the database. This is shorthand for some common sequences, which may be useful
    ///
    /// This is the least stable of the APIs, and definitely needs some usage
    pub fn update(&mut self, action: &dyn Fn(T) -> Result<T>) -> Result<T> {
        let input = self.load()?;
        let output = action(input)?;
        self.save(&output)?;
        Ok(output)
    }
}

/// ReadonlySingleton only requires a ReadonlyStorage and exposes only the
/// methods of Singleton that don't modify state.
pub struct ReadonlySingleton<'a, S: ReadonlyStorage, T>
where
    T: Serialize + DeserializeOwned + NamedType,
{
    storage: &'a S,
    key: Vec<u8>,
    // see https://doc.rust-lang.org/std/marker/struct.PhantomData.html#unused-type-parameters for why this is needed
    data: PhantomData<&'a T>,
}

impl<'a, S: ReadonlyStorage, T> ReadonlySingleton<'a, S, T>
where
    T: Serialize + DeserializeOwned + NamedType,
{
    pub fn new(storage: &'a S, key: &[u8]) -> Self {
        ReadonlySingleton {
            storage,
            key: key_prefix(key),
            data: PhantomData,
        }
    }

    /// load will return an error if no data is set at the given key, or on parse error
    pub fn load(&self) -> Result<T> {
        // TODO: replace with NotFound with cosmwasm 0.6.1
        self.may_load()?.context(ContractErr {
            msg: "uninitialized data",
        })
    }

    /// may_load will parse the data stored at the key if present, returns Ok(None) if no data there.
    /// returns an error on issues parsing
    pub fn may_load(&self) -> Result<Option<T>> {
        match self.storage.get(&self.key) {
            Some(d) => from_slice(&d).context(ParseErr {
                kind: T::short_type_name(),
            }),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use cosmwasm::mock::MockStorage;
    use named_type_derive::NamedType;
    use serde::{Deserialize, Serialize};

    use cosmwasm::errors::{Error, Unauthorized};

    #[derive(Serialize, Deserialize, NamedType, PartialEq, Debug)]
    struct Config {
        pub owner: String,
        pub max_tokens: i32,
    }

    #[test]
    fn save_and_load() {
        let mut store = MockStorage::new();
        let mut single = Singleton::<_, Config>::new(&mut store, b"config");

        assert!(single.load().is_err());
        assert_eq!(single.may_load().unwrap(), None);

        let cfg = Config {
            owner: "admin".to_string(),
            max_tokens: 1234,
        };
        single.save(&cfg).unwrap();

        assert_eq!(cfg, single.load().unwrap());
    }

    #[test]
    fn isolated_reads() {
        let mut store = MockStorage::new();
        let mut writer = singleton::<_, Config>(&mut store, b"config");

        let cfg = Config {
            owner: "admin".to_string(),
            max_tokens: 1234,
        };
        writer.save(&cfg).unwrap();

        let reader = singleton_read::<_, Config>(&store, b"config");
        assert_eq!(cfg, reader.load().unwrap());

        let other_reader = singleton_read::<_, Config>(&store, b"config2");
        assert_eq!(other_reader.may_load().unwrap(), None);
    }

    #[test]
    fn update_success() {
        let mut store = MockStorage::new();
        let mut writer = singleton::<_, Config>(&mut store, b"config");

        let cfg = Config {
            owner: "admin".to_string(),
            max_tokens: 1234,
        };
        writer.save(&cfg).unwrap();

        let output = writer.update(&|mut c| {
            c.max_tokens *= 2;
            Ok(c)
        });
        let expected = Config {
            owner: "admin".to_string(),
            max_tokens: 2468,
        };
        assert_eq!(output.unwrap(), expected);
        assert_eq!(writer.load().unwrap(), expected);
    }

    #[test]
    fn update_failure() {
        let mut store = MockStorage::new();
        let mut writer = singleton::<_, Config>(&mut store, b"config");

        let cfg = Config {
            owner: "admin".to_string(),
            max_tokens: 1234,
        };
        writer.save(&cfg).unwrap();

        let output = writer.update(&|_c| Unauthorized {}.fail());
        match output {
            Err(Error::Unauthorized { .. }) => {}
            _ => panic!("Unexpected output: {:?}", output),
        }
        assert_eq!(writer.load().unwrap(), cfg);
    }
}
