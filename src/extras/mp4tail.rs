use error_chain::error_chain;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::PathBuf;

error_chain! {
    foreign_links {
        Fmt(::std::fmt::Error);
        Io(::std::io::Error);
        Parse(std::num::ParseIntError);
        //Url(url::ParseError);
    }

}

#[derive(Default)]
pub struct Mp4TailResults {
    pub last_sequence_offset: Option<u64>,
    pub last_sequence_number: Option<i64>,
    pub last_ftyp_offset: Option<u64>,
}

pub fn check_mp4(file_path: &PathBuf, verbose: bool) -> Result<Mp4TailResults> {
    let mut file =
        std::fs::File::open(file_path).chain_err(|| format!("open file {:?}", file_path))?;

    let flen_start = file.metadata()?.len();

    let offset_search: usize = 4; // length of mp4 atom (length) prefix

    let tail_len: u64 = 10 * 1024 * 1024;
    let tail_len: u64 = tail_len.min(flen_start - offset_search as u64);
    file.seek(SeekFrom::Start(flen_start - tail_len))?;
    let tail_start: u64 = file.stream_position()?;
    if verbose {
        println!("searching from {}", tail_start);
    }

    let mut buf = alloc_u8_vec(tail_len as usize + offset_search);
    let nread = file.read_to_end(&mut buf)?;
    if verbose {
        println!("read {} bytes", nread);
    }

    let ftyp_poss: Vec<usize> = memchr::memmem::find_iter(&buf[offset_search..], "ftyp").collect();

    let mut results = Mp4TailResults::default();

    for (i, it_ftyp_pos) in ftyp_poss.iter().enumerate() {
        let ftyp_bufpos = it_ftyp_pos + offset_search;
        let next_ftyp_bufpos = ftyp_poss.get(i + 1);
        if verbose {
            println!(
                "[ftyp at {}] found at pos:{}",
                ftyp_bufpos as u64 + tail_start,
                ftyp_bufpos as u64,
            );
        }
        let moov_offset = moov_pos(&buf[ftyp_bufpos..(ftyp_bufpos + 50).min(buf.len())]);
        if moov_offset > 30 {
            if verbose {
                println!(
                    "[ftyp at {}] moov not found:{} (should be == 28 or 24)",
                    ftyp_bufpos as u64 + tail_start,
                    moov_offset
                );
            }
            continue;
        }

        let nbuf = &buf[(ftyp_bufpos - 4)..];
        let (nbuf, atom_len) = nom_u32(nbuf).map_err(|_| "can't read u32 len of atom")?;
        let nbuf = &nbuf[nbuf.len().min(atom_len as usize)..];
        nom_tag_moov(nbuf).map_err(|_| "tag moov not found")?;

        results.last_ftyp_offset = Some(ftyp_bufpos as u64 + tail_start);

        let seq_num_needle = "Sequence-Number: ";
        let mut it_sequence_num = memchr::memmem::find_iter(&buf[ftyp_bufpos..], seq_num_needle);
        if let Some(it_seq_pos) = it_sequence_num.next() {
            let buf_seq_pos = it_seq_pos + ftyp_bufpos;
            let is_seq_num_valid = if let Some(&next_ftyp_bufpos) = next_ftyp_bufpos {
                if buf_seq_pos > next_ftyp_bufpos {
                    if verbose {
                        println!(
                            "[ftyp at {}] warn: sequence number {} is after the next ftyp",
                            ftyp_bufpos as u64 + tail_start,
                            seq_num_needle
                        );
                    }
                    false
                } else {
                    true
                }
            } else {
                true
            };

            if is_seq_num_valid {
                if verbose {
                    println!(
                        "[ftyp at {}] found {} at pos:{}  filepos:{}",
                        ftyp_bufpos as u64 + tail_start,
                        seq_num_needle,
                        buf_seq_pos,
                        buf_seq_pos as u64 + tail_start
                    );
                }

                results.last_sequence_offset = Some(buf_seq_pos as u64 + tail_start);

                let seq_num_str = String::from_utf8_lossy(
                    &buf[buf_seq_pos + seq_num_needle.len()..(buf_seq_pos + 50).min(buf.len())],
                );
                results.last_sequence_number =
                    if let Some(seq_num_str) = seq_num_str.split("\r").next() {
                        Some(seq_num_str.parse()?)
                    } else {
                        None
                    };
            }
        }
    }

    let flen_finish = file.metadata()?.len();
    if flen_finish != flen_start {
        println!(
            "warn: file size changed while reading from {} to {}",
            flen_start, flen_finish
        );
    }

    Ok(results)
}

fn nom_u32(s: &[u8]) -> std::result::Result<(&[u8], u32), ()> {
    nom::number::complete::be_u32(s).map_err(|_: nom::Err<nom::error::Error<&[u8]>>| ())
}
fn nom_tag_moov(input: &[u8]) -> std::result::Result<(&[u8], &[u8]), ()> {
    nom::bytes::complete::tag("moov")(input).map_err(|_: nom::Err<nom::error::Error<&[u8]>>| ())
    //nom::bytes::complete::take("moov")(s).map_err(|_: nom::Err<nom::error::Error<&[u8]>>| ())
}

fn moov_pos(buf: &[u8]) -> usize {
    let it_moov = memchr::memmem::find_iter(buf, "moov").next();

    if let Some(it_moov_pos) = it_moov {
        it_moov_pos
    } else {
        usize::MAX
    }
}

fn alloc_u8_vec(len: usize) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::with_capacity(len);
    buf.resize(len, 0);
    buf
}
