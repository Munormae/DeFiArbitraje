// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;
interface IQuoterV2 {
    function quoteExactInputSingle(
        address tokenIn,
        address tokenOut,
        uint24 fee,
        uint256 amountIn,
        uint160 sqrtPriceLimitX96
    ) external returns (uint256 amountOut, uint160, uint32, uint256);
}
