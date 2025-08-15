// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../contracts/Executor.sol";

contract Deploy is Script {
    function run() external {
        uint256 pk = vm.envUint("PRIVATE_KEY");
        address permit2 = vm.envAddress("PERMIT2_ADDRESS");
        vm.startBroadcast(pk);
        Executor exec = new Executor(permit2);
        vm.stopBroadcast();
        console2.log("Executor deployed at:", address(exec));
    }
}
