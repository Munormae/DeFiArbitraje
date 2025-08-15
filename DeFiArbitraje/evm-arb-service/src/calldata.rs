use anyhow::Result;
use ethers::abi::{self, Token};
use ethers::types::{Address, Bytes, U256};

#[derive(Clone, Debug)]
pub enum LegKind {
    V2 {
        router: Address,
        path: Vec<Address>,
    },
    V3 {
        router: Address,
        token_in: Address,
        token_out: Address,
        fee_bps: u32,
    },
    Solidly {
        router: Address,
        pair: Address,
        stable: bool,
        token_in: Address,
    },
}

#[derive(Clone, Debug)]
pub struct LegQuote {
    pub kind: LegKind,
}

pub fn encode_route_calldata(legs: &[LegQuote], amount_in: U256, min_out: U256) -> Result<Bytes> {
    let mut tokens: Vec<Token> = Vec::new();
    tokens.push(Token::Uint(amount_in));
    tokens.push(Token::Uint(min_out));
    tokens.push(Token::Uint(U256::from(legs.len() as u64)));

    for leg in legs {
        match &leg.kind {
            LegKind::V2 { router, path } => {
                tokens.push(Token::Uint(U256::from(1u8)));
                tokens.push(Token::Address(*router));
                let path_tokens: Vec<Token> = path.iter().map(|a| Token::Address(*a)).collect();
                tokens.push(Token::Array(path_tokens));
            }
            LegKind::V3 {
                router,
                token_in,
                token_out,
                fee_bps,
            } => {
                tokens.push(Token::Uint(U256::from(2u8)));
                tokens.push(Token::Address(*router));
                tokens.push(Token::Address(*token_in));
                tokens.push(Token::Address(*token_out));
                tokens.push(Token::Uint(U256::from(*fee_bps)));
            }
            LegKind::Solidly {
                router,
                pair,
                stable,
                token_in,
            } => {
                tokens.push(Token::Uint(U256::from(3u8)));
                tokens.push(Token::Address(*router));
                tokens.push(Token::Address(*pair));
                tokens.push(Token::Bool(*stable));
                tokens.push(Token::Address(*token_in));
            }
        }
    }

    Ok(Bytes::from(abi::encode(&tokens)))
}
