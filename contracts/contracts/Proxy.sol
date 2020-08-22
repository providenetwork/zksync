pragma solidity ^0.5.0;

import "./Ownable.sol";
import "./Upgradeable.sol";
import "./UpgradeableMaster.sol";


/// @title Proxy Contract
/// @dev NOTICE: Proxy must implement UpgradeableMaster interface to prevent calling some function of it not by master of proxy
/// @author Matter Labs
contract Proxy is Upgradeable, UpgradeableMaster, Ownable {

    /// @notice Storage position of "target" (actual implementation address: keccak256('eip1967.proxy.implementation') - 1)
    bytes32 private constant targetPosition = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;

    ///@notice this initializes the global value of Process_type to 0. This is only changed when the proxy for zkynch is called.
    bytes32 private Process_type; 

    /// @notice Contract constructor
    /// @dev Calls Ownable contract constructor and initialize target
    /// @param target Initial implementation address
    /// @param targetInitializationParameters Target initialization parameters
    constructor(address target, bytes memory targetInitializationParameters) Ownable(msg.sender) public {
        setTarget(target);
        (bool initializationSuccess, ) = getTarget().delegatecall(
            abi.encodeWithSignature("initialize(bytes)", targetInitializationParameters)
        );
        require(initializationSuccess, "uin11"); // uin11 - target initialization failed
    }

    /// @notice Intercepts initialization calls
    function initialize(bytes calldata) external pure {
        revert("ini11"); // ini11 - interception of initialization call
    }

    /// @notice Intercepts upgrade calls
    function upgrade(bytes calldata) external pure {
        revert("upg11"); // upg11 - interception of upgrade call
    }

    /// @notice Returns target of contract
    /// @param bytes32 process_type, if process_type is empty string then target is either verifier or goverance, if it is not zero then load the target address based on process_type
    /// @return Actual implementation address
    function getTarget(bytes32 process_type) public view returns (address target) {
        
        if (process_type != 0) {
         bytes32 position = process_type;
        } else {
            bytes32 position = targetPosition;
        }

        assembly {
            target := sload(position)
        }
    }

    /// @notice Update!! Sets new target of contract
    /// @param _newTarget New actual implementation address
    /// @param bytes32 process_type, if process_type is empty string then target is either verifier or goverance, if it is not zero then load the target address based on process_type
    function setTarget(bytes32 process_type, address _newTarget) internal {
        if (process_type != 0) {
         bytes32 position = process_type;
        } else {
            bytes32 position = targetPosition;
        }

        assembly {
            sstore(position, _newTarget)
        }
    }

    /// @notice Upgrades target
    /// @param newTarget New target
    /// @param newTargetUpgradeParameters New target upgrade parameters
    /// @param bytes32 process_type specifices process type if upgrade is for a zksynch process contract
    function upgradeTarget(address newTarget, bytes32 process_type, bytes calldata newTargetUpgradeParameters) external {
        requireMaster(msg.sender);

        setTarget(process_type, newTarget);
        (bool upgradeSuccess, ) = getTarget(process_type).delegatecall(
            abi.encodeWithSignature("upgrade(bytes)", newTargetUpgradeParameters)
        );
        require(upgradeSuccess, "ufu11"); // ufu11 - target upgrade failed
    }

    /// @notice New!! In order to use 1 proxy contract for multiple zksynch process contracts 
    /// we need to call the fallback function from within the Proxy contract. bytes32 Process_type will be set 
    /// to the relevant process_type (0 is for either Governance and Verifier and other types are for the different zksynch processes)
    /// this allows us to use one type of proxy contract and one type of proxy can manage several taregt contracts
    /// @param bytes32 process_type spcifies the process type and thus the target contract
    /// @param bytes payload spcifies the payload for the proxy call in the contract fallback function
    function proxyCall (bytes32 process_type, bytes calldata payload) external {

        Process_type = process_type;

        address(this).call(payload);

    }  

    /// @notice UPDATED!! Performs a delegatecall to the contract implementation
    /// @dev Fallback function allowing to perform a delegatecall to the given implementation
    /// This function will return whatever the implementation call returns
    function() external payable {
        require (msg.sender == address(this));
        address _target = getTarget(Process_Type);
        assembly {
            // The pointer to the free memory slot
            let ptr := mload(0x40)
            // Copy function signature and arguments from calldata at zero position into memory at pointer position
            calldatacopy(ptr, 0x0, calldatasize)
            // Delegatecall method of the implementation contract, returns 0 on error
            let result := delegatecall(
                gas,
                _target,
                ptr,
                calldatasize,
                0x0,
                0
            )
            // Get the size of the last return data
            let size := returndatasize
            // Copy the size length of bytes from return data at zero position to pointer position
            returndatacopy(ptr, 0x0, size)
            // Depending on result value
            switch result
            case 0 {
                // End execution and revert state changes
                revert(ptr, size)
            }
            default {
                // Return data with length of size at pointers position
                return(ptr, size)
            }
        }
    }

    /// UpgradeableMaster functions -- ALL UPDATED

    /// @notice Notice period before activation preparation status of upgrade mode
    /// @param bytes32 process_type specifices process type if upgrade is for a zksynch process contract
    function getNoticePeriod(bytes32 process_type) external returns (uint) {
        (bool success, bytes memory result) = getTarget(process_type).delegatecall(abi.encodeWithSignature("getNoticePeriod()"));
        require(success, "unp11"); // unp11 - upgradeNoticePeriod delegatecall failed
        return abi.decode(result, (uint));
    }

    /// @notice Notifies proxy contract that notice period started
    /// @param bytes32 process_type specifices process type if upgrade is for a zksynch process contract
    function upgradeNoticePeriodStarted(bytes32 process_type) external {
        requireMaster(msg.sender);
        (bool success, ) = getTarget(process_type).delegatecall(abi.encodeWithSignature("upgradeNoticePeriodStarted()"));
        require(success, "nps11"); // nps11 - upgradeNoticePeriodStarted delegatecall failed
    }

    /// @notice Notifies proxy contract that upgrade preparation status is activated
    /// @param bytes32 process_type specifices process type if upgrade is for a zksynch process contract
    function upgradePreparationStarted(bytes32 process_type) external {
        requireMaster(msg.sender);
        (bool success, ) = getTarget(process_type).delegatecall(abi.encodeWithSignature("upgradePreparationStarted()"));
        require(success, "ups11"); // ups11 - upgradePreparationStarted delegatecall failed
    }

    /// @notice Notifies proxy contract that upgrade canceled
    /// @param bytes32 process_type specifices process type if upgrade is for a zksynch process contract
    function upgradeCanceled(bytes32 process_type) external {
        requireMaster(msg.sender);
        (bool success, ) = getTarget(process_type).delegatecall(abi.encodeWithSignature("upgradeCanceled()"));
        require(success, "puc11"); // puc11 - upgradeCanceled delegatecall failed
    }

    /// @notice Notifies proxy contract that upgrade finishes
    /// @param bytes32 process_type specifices process type if upgrade is for a zksynch process contract
    function upgradeFinishes(bytes32 process_type) external {
        requireMaster(msg.sender);
        (bool success, ) = getTarget(process_type).delegatecall(abi.encodeWithSignature("upgradeFinishes()"));
        require(success, "puf11"); // puf11 - upgradeFinishes delegatecall failed
    }

    /// @notice Checks that contract is ready for upgrade
    /// @return bool flag indicating that contract is ready for upgrade
    /// @param bytes32 process_type specifices process type if upgrade is for a zksynch process contract
    function isReadyForUpgrade(bytes32 process_type) external returns (bool) {
        (bool success, bytes memory result) = getTarget(process_type).delegatecall(abi.encodeWithSignature("isReadyForUpgrade()"));
        require(success, "rfu11"); // rfu11 - readyForUpgrade delegatecall failed
        return abi.decode(result, (bool));
    }

}
