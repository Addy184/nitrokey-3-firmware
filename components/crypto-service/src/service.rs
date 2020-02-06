use core::convert::TryInto;

use crate::api::*;
use crate::config::*;
use crate::error::Error;
use crate::types::*;

pub use crate::pipe::ServiceEndpoint;

use chacha20poly1305::ChaCha8Poly1305;
pub use embedded_hal::blocking::rng::Read as RngRead;

// associated keys end up namespaced under "/fido2"
// example: "/fido2/keys/2347234"
// let (mut fido_endpoint, mut fido2_client) = Client::new("fido2");
// let (mut piv_endpoint, mut piv_client) = Client::new("piv");

pub struct ServiceResources<'s, Rng, PersistentStorage, VolatileStorage>
where
    Rng: RngRead,
    PersistentStorage: LfsStorage,
    VolatileStorage: LfsStorage,
{
    rng: Rng,
    // maybe make this more flexible later, but not right now
    // cryptoki: "token objects"
    pfs: FilesystemWith<'s, 's, PersistentStorage>,
    // cryptoki: "session objects"
    vfs: FilesystemWith<'s, 's, VolatileStorage>,
}

pub struct Service<'a, 's, Rng, PersistentStorage, VolatileStorage>
where
    Rng: RngRead,
    PersistentStorage: LfsStorage,
    VolatileStorage: LfsStorage,
{
    eps: Vec<ServiceEndpoint<'a>, MAX_SERVICE_CLIENTS>,
    resources: ServiceResources<'s, Rng, PersistentStorage, VolatileStorage>,
}

impl<'s, R: RngRead, P: LfsStorage, V: LfsStorage> ServiceResources<'s, R, P, V> {

    // TODO: key a `/root/aead-key`
    pub fn get_aead_key(&self) -> Result<AeadKey, Error> {
        Ok([37u8; 32])
    }

    // TODO: key a `/root/aead-nonce` counter (or use entropy?)
    pub fn get_aead_nonce(&self) -> Result<AeadNonce, Error> {
        Ok([42u8; 12])
    }

    // global choice of algorithm: we do Chacha8Poly1305 here
    // TODO: oh how annoying these GenericArrays
    pub fn aead_in_place(&mut self, ad: &[u8], buf: &mut [u8]) -> Result<(AeadNonce, AeadTag), Error> {
        use chacha20poly1305::aead::{Aead, NewAead};

        // keep in state?
        let aead = ChaCha8Poly1305::new(GenericArray::clone_from_slice(&self.get_aead_key()?));
        // auto-increments
        let nonce = self.get_aead_nonce()?;

        // aead.encrypt_in_place_detached(&nonce, ad, buf).map(|g| g.as_slice().try_into().unwrap())?;
        // not sure what can go wrong with AEAD
        let tag: AeadTag = aead.encrypt_in_place_detached(
            &GenericArray::clone_from_slice(&nonce), ad, buf
        ).unwrap().as_slice().try_into().unwrap();
        Ok((nonce, tag))
    }

    pub fn adad_in_place(&mut self, nonce: &AeadNonce, ad: &[u8], buf: &mut [u8], tag: &AeadTag) -> Result<(), Error> {
        use chacha20poly1305::aead::{Aead, NewAead};

        // keep in state?
        let aead = ChaCha8Poly1305::new(GenericArray::clone_from_slice(&self.get_aead_key()?));

        aead.decrypt_in_place_detached(
            &GenericArray::clone_from_slice(nonce),
            ad,
            buf,
            &GenericArray::clone_from_slice(tag)
        ).map_err(|e| Error::AeadError)
    }

    pub fn reply_to(&mut self, request: Request) -> Result<Reply, Error> {
        match request {
            Request::DummyRequest => {
                #[cfg(test)]
                println!("got a dummy request!");
                Ok(Reply::DummyReply)
            },

            // NOT TODO: use the `?` operator <-- THIS DOES NOT WORK BY "TYPE THEORY"
            // (reason: resources could (pathologically at least) have same error types,
            // compiler could not know which From to apply)
            //
            // TODO: how to handle queue failure?
            // TODO: decouple this in such a way that we can easily extend the
            //       cryptographic capabilities on two axes:
            //        - mechanisms
            //        - backends
            Request::GenerateKeypair(request) => {
                match request.mechanism {
                    Mechanism::Ed25519 => {
                        self.generate_ed25519_keypair(request)
                    },

                    #[allow(unreachable_patterns)]
                    _ => {
                        Err(Error::MechanismNotAvailable)
                    }
                }
            },

            _ => {
                #[cfg(test)]
                println!("todo: {:?} request!", &request);
                Err(Error::RequestNotAvailable)
            },
        }
    }

    pub fn generate_ed25519_keypair(&mut self, request: request::GenerateKeypair) -> Result<Reply, Error> {
        // generate key
        let mut seed = [0u8; 32];
        self.rng.read(&mut seed)
            .map_err(|_| Error::EntropyMalfunction)?;

        // not needed now.  do we want to cache its public key?
        // let keypair = salty::Keypair::from(&seed);
        // #[cfg(all(test, feature = "verbose-tests"))]
        // println!("ed25519 keypair with public key = {:?}", &keypair.public);

        // generate unique id
        let unique_id = self.generate_unique_id()?;

        let mut u = unique_id.clone().0;
        let (nonce, tag)= self.aead_in_place(&[], &mut u)?;
        #[cfg(all(test, feature = "verbose-tests"))]
        println!("aead: encrypted unique id = {:?}, nonce = {:?}, tag = {:?}", &u, &nonce, &tag);

        // store key
        // TODO: add "app" namespacing, and AEAD this ID
        // let mut path = [0u8; 38];
        // path[..6].copy_from_slice(b"/test/");
        // format_hex(&unique_id, &mut path[6..]);
        let mut path = [0u8; 33];
        path[..1].copy_from_slice(b"/");
        path[1..].copy_from_slice(&unique_id.hex());

        self.store_serialized_key(&path, &seed)?;

        // return key handle
        Ok(Reply::GenerateKey(reply::GenerateKey {
            key_handle: KeyHandle { key_id: unique_id }
        }))
    }

    pub fn generate_unique_id(&mut self) -> Result<UniqueId, Error> {
        let mut unique_id = [0u8; 16];

        self.rng.read(&mut unique_id)
            .map_err(|_| Error::EntropyMalfunction)?;

        #[cfg(all(test, feature = "verbose-tests"))]
        println!("unique id {:?}", &unique_id);
        Ok(UniqueId(unique_id))
    }

    pub fn store_serialized_key(&mut self, path: &[u8], serialized_key: &[u8]) -> Result<(), Error> {
        #[cfg(test)]
        // actually safe, as path is ASCII by construction
        println!("storing in file {:?}", unsafe { core::str::from_utf8_unchecked(&path[..]) });

        use littlefs2::fs::{File, FileWith};
        let mut alloc = File::allocate();
        let mut file = FileWith::create(&path[..], &mut alloc, &mut self.vfs)
            .map_err(|_| Error::FilesystemWriteFailure)?;
        use littlefs2::io::WriteWith;
        file.write(&serialized_key)
            .map_err(|_| Error::FilesystemWriteFailure)?;
        file.sync()
            .map_err(|_| Error::FilesystemWriteFailure)?;

        Ok(())
    }
}

impl<'a, 's, R: RngRead, P: LfsStorage, V: LfsStorage> Service<'a, 's, R, P, V> {

    pub fn new(
        rng: R,
        persistent_storage: FilesystemWith<'s, 's, P>,
        volatile_storage: FilesystemWith<'s, 's, V>,
    )
        -> Self
    {
        Self {
            eps: Vec::new(),
            resources: ServiceResources {
                rng,
                pfs: persistent_storage,
                vfs: volatile_storage,
            },
        }
    }

    pub fn add_endpoint(&mut self, ep: ServiceEndpoint<'a>) -> Result<(), ServiceEndpoint> {
        self.eps.push(ep)
    }

    // process one request per client which has any
    pub fn process(&mut self) {
        // split self since we iter-mut over eps and need &mut of the other resources
        let mut eps = &mut self.eps;
        let mut resources = &mut self.resources;

        for ep in eps.iter_mut() {
            if !ep.send.ready() {
                continue;
            }
            if let Some(request) = ep.recv.dequeue() {
                #[cfg(test)]
                println!("service got request: {:?}", &request);
                let reply_result = resources.reply_to(request);
                ep.send.enqueue(reply_result).ok();
            }
        }
    }
}

