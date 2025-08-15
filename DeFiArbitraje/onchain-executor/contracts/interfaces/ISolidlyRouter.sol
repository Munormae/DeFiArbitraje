// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;
interface ISolidlyRouter {
    function swapExactTokensForTokensSimple(
        uint amountIn, uint amountOutMin,
        address tokenFrom, address tokenTo,
        bool stable, address to, uint deadline
    ) external returns (uint amountOut);
}
