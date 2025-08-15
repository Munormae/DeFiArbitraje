// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;
interface ISolidlyPair {
    function getAmountOut(uint amountIn, address tokenIn) external view returns (uint amountOut);
}
