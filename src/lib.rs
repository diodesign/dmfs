/* Diosix Manifest File System
 * 
 * This very simple read-only file system is intended to be embedded
 * in a diosix hypervisor build, and parsed and unpacked at run-time.
 * It enables the hypervisor to ship with essential services and guests
 * needed to boot a fully functional system from storage.
 * 
 * Typically, a DMFS image will be created on a build host, and parsed on a target device.
 * The image should contain as objects the system services necessary to aid the hypervisor,
 * any welcome text to output via the debug channel, and guest OSes to start.
 * 
 * To create a DMFS image:
 * 1. Create a Manifest object and populate it using add()
 *    Note you shouldn't include an EndOfList object: that's added automatically.
 * 2. Call Manifest::to_image() to create the file system image as a byte array
 * 
 * Note: filenames are only checked at to_image()
 * 
 * To read a DMFS image in memory:
 * 1. call ManifestImageIter::from_slice() using a byte slice of the dmfs image in memory
 * 2. Iterate the ManifestImageIter to get its contents as a series of manifest objects
 * 
 * This does not require the standard library though it does require a heap allocator.
 * 
 * (c) Chris Williams, 2021.
 *
 * See LICENSE for usage and copying.
 */

/* the manifest binary file format is real simple, and is as follows:

   u32: magic header that must equal MANIFEST_MAGIC.
        the target system's word width must match the dmfs image's magic.
   u32: version integer. Must be equal or less than MANIFEST_VERSION

   then multiple sequential blocks, each representing an object, of:

   u32: magic header that must equal MANIFEST_OBJECT_MAGIC
   u32: object type, chosen from one of ManifestObjectType.
        if the type is EndOfList then the image stops here.
   u64: size of object's contents in bytes.
   <NULL-terminated string of this object's unique name>
   <NULL-terminated string describing this object>
   <NULL padding to next 32-bit word>
   u32: number of properties (N) granted to this object
   N x <NULL-terminated property strings>
   <NULL padding to next 64-bit word>
   <contents of the object as a byte stream>
   <NULL padding to next 64-bit word>

    the object name could be a filename, or something other code can use to identify it.
    it should be unique among all the other objects in the dmfs image.

    TODO: replace this with serial-deserialization, liek serdes? 
*/

#![no_std]
#![allow(dead_code)]

extern crate alloc;
extern crate byterider;

use core::mem::size_of;
use alloc::vec::Vec;
use alloc::string::String;
use byterider::Bytes;
use core::ops::Range;

/* manifest image must start with the following */
const MANIFEST_MAGIC: u32 = 0xD105C001;
const MANIFEST_OBJECT_MAGIC: u32 = 0xD1015D4D;
const MANIFEST_VERSION: u32 = 2; /* version supported */

/* version history
   1 = 32-bit object content padding and 32-bit object sizes
   2 = 64-bit object content padding and 64-bit object sizes
*/

#[derive(Debug)]
pub enum ManifestError
{
    MalformedHeader, /* header is too small or malformed */
    BadMagic, /* unrecognized magic number in dmfs image header */
    VersionMismatch, /* dmfs image is of a later version than this code is aware of */
    CantUseRegionHere /* trying to use a region of an image to generate an image */
}

#[derive(Debug, Clone, Copy)]
pub enum ManifestObjectType
{
    BootMsg, /* a textfile to output to the hypervisor's debug channel during startup */
    SystemService, /* executable application to be run at startup */
    GuestOS, /* executable guest OS to be loaded later */
    Unknown, /* reserved for unrecognized types */
    EndOfList /* signify we're at the end of the image */
}

impl ManifestObjectType
{
    /* convert an object type to an integer */
    pub fn to_integer(&self) -> u32
    {
        match self
        {
            ManifestObjectType::EndOfList     => 0,
            ManifestObjectType::BootMsg       => 1,
            ManifestObjectType::SystemService => 2,
            ManifestObjectType::GuestOS       => 3,
            ManifestObjectType::Unknown       => 4
        }
    }

    pub fn from_integer(nr: u32) -> ManifestObjectType
    {
        match nr
        {
            0 => ManifestObjectType::EndOfList,
            1 => ManifestObjectType::BootMsg,
            2 => ManifestObjectType::SystemService,
            3 => ManifestObjectType::GuestOS,
            4 | _ => ManifestObjectType::Unknown,
        }
    }
}

/* define the source of an object's data: an independent array, or a chunk of the image in memory
   Bytes should be used when generating an image from outside data.
   Bytes and Region can be used when parsing an image from memory. */
pub enum ManifestObjectData
{
    Bytes(Vec<u8>), /* contents as a separate byte vector */
    Region(Range<usize>) /* start and end indexes into the manifest image in memory */
}

impl ManifestObjectData
{
    pub fn len(&self) -> usize
    {
        match self
        {
            ManifestObjectData::Bytes(v) => v.len(),
            ManifestObjectData::Region(r) => r.end - r.start
        }
    }
}

/* describe an object to be added to or already in a manifest */
pub struct ManifestObject
{
    objtype: ManifestObjectType, /* type of the object */
    name: String, /* unique identifier for this object */
    descr: String, /* user-friendly description of this object */
    properties: Vec<String>, /* list of properties granted to this object */
    data: ManifestObjectData /* contents of the object */
}

impl ManifestObject
{
    /* create an object to add to a manifest
       => objtype = object type
          name = unique name for the object
          descr = user-friendly description of this object
          data = object contents
          props = array of properties assigned to the object, or None */
    pub fn new(objtype: ManifestObjectType, name: String, descr: String, data: ManifestObjectData, properties: Option<Vec<String>>) -> ManifestObject
    {
        ManifestObject
        {
            objtype,
            name,
            descr,
            data,
            properties: match properties
            {
                Some(p) => p,
                None => Vec::new()
            }
        }
    }

    pub fn get_type(&self) -> ManifestObjectType { self.objtype }
    pub fn get_name(&self) -> String { self.name.clone() }
    pub fn get_description(&self) -> String { self.descr.clone() }
    pub fn get_properties(&self) -> Vec<String> { self.properties.clone() }
    pub fn get_contents(&self) -> &ManifestObjectData { &self.data }
    pub fn get_contents_size(&self) -> usize { self.data.len() }
}

/* high-level definition of a system manifest */
pub struct Manifest
{
    /* list of objects to include */
    objects: Vec<ManifestObject>
}

impl Manifest
{
    /* create an empty manifest */
    pub fn new() -> Manifest
    {
        Manifest
        {
            objects: Vec::new()
        }
    }

    pub fn add(&mut self, to_add: ManifestObject)
    {
        self.objects.push(to_add);
    }

    pub fn to_image(&self) -> Result<Bytes, ManifestError>
    {
        /* create the holding object for the image's binary data
        and start with the magic and version metadata */
        let mut b = Bytes::new();
        b.add_u32(MANIFEST_MAGIC);
        b.add_u32(MANIFEST_VERSION);

        for object in &self.objects
        {
            /* include magic for integrity check reasons */
            b.add_u32(MANIFEST_OBJECT_MAGIC);

            /* stream out the object data */
            b.add_u32(object.get_type().to_integer());
            b.add_u64(object.get_contents_size() as u64);
            b.add_null_term_string(object.get_name().as_str());
            b.add_null_term_string(object.get_description().as_str());
            b.pad_to_u32();

            /* output number of properties (N) assigned to this object
               and then write out N properties as null-term'd strings */
            b.add_u32(object.properties.len() as u32);
            for property in &object.properties
            {
                b.add_null_term_string(property.as_str());   
            }
            b.pad_to_u64();

            /* copy object bytes into the image */
            match object.get_contents()
            {
                ManifestObjectData::Bytes(bytes) =>
                {
                    for byte in bytes
                    {
                        b.add_u8(*byte);
                    }
                    b.pad_to_u64();
                },

                _ => return Err(ManifestError::CantUseRegionHere)
            }
        }

        /* add the bookend type */
        b.add_u32(ManifestObjectType::EndOfList.to_integer());

        Ok(b)
    }
}

/* define an iterator over a manifest image in memory */
pub struct ManifestImageIter
{
    offset: usize,
    bytes: Bytes,
}

impl ManifestImageIter
{
    /* create manifest image in memory from byte slice */
    pub fn from_slice(blob: &[u8]) -> Result<ManifestImageIter, ManifestError>
    {
        /* this is horrendously expensive. FIXME: can we do this without copying MBs of data? */
        let bytes = Bytes::from_slice(blob);
        let mut offset = 0;
        let width = size_of::<u32>();

        /* compliance checks */
        match bytes.read_u32(offset)
        {
            Some(magic) => if magic != MANIFEST_MAGIC
            {
                return Err(ManifestError::BadMagic);
            }
            else
            {
                offset = offset + width;
            },
            None => return Err(ManifestError::MalformedHeader)
        };

        match bytes.read_u32(offset)
        {
            Some(version) => if version > MANIFEST_VERSION
            {
                return Err(ManifestError::VersionMismatch);
            }
            else
            {
                offset = offset + width;
            },
            None => return Err(ManifestError::MalformedHeader)
        };

        Ok(ManifestImageIter
        {
            bytes,
            offset /* skip header */
        })
    }
}

/* spin through all the objects in the manifest */
impl Iterator for ManifestImageIter
{
    type Item = ManifestObject;

    fn next(&mut self) -> Option<ManifestObject>
    {
        /* pick up from where we were last at */
        let mut offset = self.offset;
        let width = size_of::<u32>();
        let double_width = size_of::<u64>();

        /* make sure the magic matches for this object, or bail */
        if self.bytes.read_u32(offset)? != MANIFEST_OBJECT_MAGIC
        {
            return None;
        }
        offset = offset + width;

        /* extract the object's meta data.
        end the iteration if we reach an EndOfList placeholder object */
        let obj_type = match ManifestObjectType::from_integer(self.bytes.read_u32(offset)?)
        {
            ManifestObjectType::EndOfList => return None,
            t => t
        };
        offset = offset + width;

        let obj_size = self.bytes.read_u64(offset)?;
        offset = offset + double_width;

        let obj_name = self.bytes.read_null_term_string(offset)?;
        offset = offset + obj_name.len() + 1; // don't forget the null byte

        let obj_desc = self.bytes.read_null_term_string(offset)?;
        offset = offset + obj_desc.len() + 1; // don't forget the null byte
        offset = Bytes::align_to_next_u32(offset);

        let obj_property_count = self.bytes.read_u32(offset)?;
        offset = offset + width;
        let mut obj_props = Vec::new();

        for _ in 0..obj_property_count
        {
            let prop_string = self.bytes.read_null_term_string(offset)?;
            offset = offset + prop_string.len() + 1; // don't forget the null byte
            obj_props.push(prop_string);
        }
        offset = Bytes::align_to_next_u64(offset);

        /* define the region of the image that contains the object's contents */
        let region = Range { start: offset, end: offset + obj_size as usize };

        /* save the offset for the next time round */
        self.offset = Bytes::align_to_next_u64(offset + obj_size as usize);

        Some(ManifestObject
        {
            objtype: obj_type,
            name: obj_name,
            descr: obj_desc,
            properties: obj_props,
            data: ManifestObjectData::Region(region)
        })
    }
}

#[cfg(test)]
mod tests
{
    #[test]
    fn it_works()
    {
        assert_eq!(2 + 2, 4);
    }
}
