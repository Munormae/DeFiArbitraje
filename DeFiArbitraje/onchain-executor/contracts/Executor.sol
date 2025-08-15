// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {ReentrancyGuard} from "./libraries/ReentrancyGuard.sol";
import {SafeTransferLib} from "./libraries/SafeTransferLib.sol";
import {IERC20} from "./interfaces/IERC20.sol";
import {IUniswapV2Router02} from "./interfaces/IUniswapV2Router02.sol";
import {IUniswapV3SwapRouter} from "./interfaces/IUniswapV3SwapRouter.sol";
import {IQuoterV2} from "./interfaces/IQuoterV2.sol";
import {ISolidlyRouter} from "./interfaces/ISolidlyRouter.sol";
import {ISolidlyPair} from "./interfaces/ISolidlyPair.sol";
import {IPermit2} from "./interfaces/IPermit2.sol";

contract Executor is ReentrancyGuard {
    using SafeTransferLib for address;

    address public owner;
    address public immutable DEFAULT_PERMIT2;

    event Executed(address indexed caller, address indexed recipient, address inputToken, uint256 amountIn, int256 profit, address lastToken);
    event OwnerChanged(address indexed oldOwner, address indexed newOwner);
    event Swept(address indexed token, address indexed to, uint256 amount);

    modifier onlyOwner() {
        require(msg.sender == owner, "ONLY_OWNER");
        _;
    }

    constructor(address _defaultPermit2) {
        owner = msg.sender;
        DEFAULT_PERMIT2 = _defaultPermit2;
    }

    function setOwner(address n) external onlyOwner { emit OwnerChanged(owner, n); owner = n; }

    struct Hop {
        uint8 protocol;        // 1=UniV2, 2=UniV3, 3=Solidly
        address router;        // V2/V3/Solidly router
        address pool;          // для Solidly: pair (simulate), иначе 0
        address quoter;        // для V3 simulate: QuoterV2
        address tokenIn;
        address tokenOut;
        uint256 amountIn;      // 0 => взять текущий баланс tokenIn
        uint24 fee;            // UniV3 fee
        bool stable;           // Solidly stable?
        uint256 minOut;        // minOut per hop
        uint256 deadline;      // deadline per hop
        uint160 sqrtPriceLimitX96; // V3 лимит цены (обычно 0)
    }

    struct Permit2Data {
        address permit2;                    // 0 => использовать DEFAULT_PERMIT2
        IPermit2.PermitTransferFrom permit;
        IPermit2.SignatureTransferDetails transferDetails; // to игнорируется, перезапишем адресом контракта
        bytes signature;
    }

    function execute(
        Hop[] calldata hops,
        address inputToken,
        uint256 amountIn,
        uint256 minProfit,
        address recipient,
        bool pullFromSender,
        Permit2Data calldata p2
    ) external nonReentrant returns (int256 profit) {
        require(hops.length > 0, "NO_HOPS");
        require(recipient != address(0), "BAD_RECIP");

        address lastToken = hops[hops.length - 1].tokenOut;
        uint256 lastBalBefore = lastToken.balanceOf(address(this));

        if (amountIn > 0) {
            if (pullFromSender) {
                _pullWithPermit2OrTransfer(inputToken, msg.sender, amountIn, p2);
            }
        }

        for (uint256 i = 0; i < hops.length; i++) {
            Hop calldata h = hops[i];
            uint256 inAmt = h.amountIn == 0 ? h.tokenIn.balanceOf(address(this)) : h.amountIn;
            require(inAmt > 0, "ZERO_IN");

            if (h.protocol == 1) {
                _approveIfNeeded(h.tokenIn, h.router, inAmt);
                address[] memory path = new address[](2);
                path[0] = h.tokenIn; path[1] = h.tokenOut;
                IUniswapV2Router02(h.router).swapExactTokensForTokens(
                    inAmt, h.minOut, path, address(this), h.deadline
                );
            } else if (h.protocol == 2) {
                _approveIfNeeded(h.tokenIn, h.router, inAmt);
                IUniswapV3SwapRouter.ExactInputSingleParams memory params =
                    IUniswapV3SwapRouter.ExactInputSingleParams({
                        tokenIn: h.tokenIn,
                        tokenOut: h.tokenOut,
                        fee: h.fee,
                        recipient: address(this),
                        deadline: h.deadline,
                        amountIn: inAmt,
                        amountOutMinimum: h.minOut,
                        sqrtPriceLimitX96: h.sqrtPriceLimitX96
                    });
                IUniswapV3SwapRouter(h.router).exactInputSingle(params);
            } else if (h.protocol == 3) {
                _approveIfNeeded(h.tokenIn, h.router, inAmt);
                ISolidlyRouter(h.router).swapExactTokensForTokensSimple(
                    inAmt, h.minOut, h.tokenIn, h.tokenOut, h.stable, address(this), h.deadline
                );
            } else {
                revert("BAD_PROTOCOL");
            }
        }

        uint256 lastBalAfter = lastToken.balanceOf(address(this));
        uint256 grossGain = lastBalAfter - lastBalBefore;
        uint256 netGain = lastToken == inputToken ? (grossGain > amountIn ? (grossGain - amountIn) : 0) : grossGain;
        require(netGain >= minProfit, "MIN_PROFIT");
        profit = int256(uint256(netGain));

        if (lastBalAfter > lastBalBefore) {
            uint256 delta = lastBalAfter - lastBalBefore;
            lastToken.safeTransfer(recipient, delta);
        }
        uint256 restIn = inputToken.balanceOf(address(this));
        if (restIn > 0) {
            inputToken.safeTransfer(recipient, restIn);
        }
        emit Executed(msg.sender, recipient, inputToken, amountIn, profit, lastToken);
    }

    function simulate(
        Hop[] calldata hops,
        address inputToken,
        uint256 amountIn
    ) external returns (uint256 expectedOut) {
        require(hops.length > 0, "NO_HOPS");
        uint256 amt = amountIn;
        for (uint256 i = 0; i < hops.length; i++) {
            Hop calldata h = hops[i];
            uint256 inAmt = h.amountIn == 0 ? amt : h.amountIn;
            if (h.protocol == 1) {
                address[] memory path = new address[](2);
                path[0] = h.tokenIn; path[1] = h.tokenOut;
                uint[] memory amts = IUniswapV2Router02(h.router).getAmountsOut(inAmt, path);
                amt = amts[1];
            } else if (h.protocol == 2) {
                (uint256 out,, ,) = IQuoterV2(h.quoter).quoteExactInputSingle(
                    h.tokenIn, h.tokenOut, h.fee, inAmt, h.sqrtPriceLimitX96
                );
                amt = out;
            } else if (h.protocol == 3) {
                amt = ISolidlyPair(h.pool).getAmountOut(inAmt, h.tokenIn);
            } else {
                revert("BAD_PROTOCOL");
            }
        }
        expectedOut = amt;
    }

    function _approveIfNeeded(address token, address spender, uint256 amount) internal {
        (bool s, bytes memory d) = token.staticcall(abi.encodeWithSelector(0xdd62ed3e, address(this), spender));
        uint256 allowance = 0;
        if (s && d.length >= 32) { allowance = abi.decode(d, (uint256)); }
        if (allowance < amount) {
            SafeTransferLib.safeApprove(token, spender, 0);
            SafeTransferLib.safeApprove(token, spender, type(uint256).max);
        }
    }

    function _pullWithPermit2OrTransfer(
        address token, address ownerAddr, uint256 amount, Permit2Data calldata p2
    ) internal {
        address permit = p2.permit2 == address(0) ? DEFAULT_PERMIT2 : p2.permit2;
        if (permit != address(0) && p2.signature.length > 0) {
            IPermit2.SignatureTransferDetails memory td = IPermit2.SignatureTransferDetails({
                to: address(this),
                requestedAmount: p2.transferDetails.requestedAmount == 0 ? amount : p2.transferDetails.requestedAmount
            });
            IPermit2(permit).permitTransferFrom(p2.permit, td, ownerAddr, p2.signature);
        } else {
            token.safeTransferFrom(ownerAddr, address(this), amount);
        }
    }

    function sweep(address token, address to) external onlyOwner {
        uint256 bal = token.balanceOf(address(this));
        token.safeTransfer(to, bal);
        emit Swept(token, to, bal);
    }
}

