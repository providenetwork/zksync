pragma solidity ^0.5.0;
pragma experimental ABIEncoderV2;

import "./SafeMath.sol";
import "./Events.sol";
import "./Ownable.sol";
import "./Upgradeable.sol";
import "./UpgradeableMaster.sol";

/// @title Upgrade Gatekeeper Contract
/// @author Matter Labs
contract UpgradeGatekeeper is UpgradeEvents, Ownable {
    using SafeMath for uint256;


    ///@notice NEW!! struct that collects upgradeable contracts for zksynch, governance and verifier
    struct managedContracts {
        Upgradeable governance;
        Upgradeable verifier;
        bytes32[] Process_type;
        Upgradeable[] zksynchcontracts;
    };   
    

    /// @notice Upgrade mode statuses
    enum UpgradeStatus {
        Idle,
        NoticePeriod,
        Preparation
    }

    UpgradeStatus public upgradeStatus;

    /// @notice Notice period finish timestamp (as seconds since unix epoch)
    /// @dev Will be equal to zero in case of not active upgrade mode
    uint public noticePeriodFinishTimestamp;

    /// @notice Addresses of the next versions of the contracts to be upgraded (if element of this array is equal to zero address it means that appropriate upgradeable contract wouldn't be upgraded this time)
    /// @dev Will be empty in case of not active upgrade mode
    managedContracts public nextTargets;

    /// @notice Version id of contracts
    uint public versionId;

    /// @notice Contract which defines notice period duration and allows finish upgrade during preparation of it
    UpgradeableMaster public mainContract;

    /// @notice Contract constructor
    /// @param _mainContract Contract which defines notice period duration and allows finish upgrade during preparation of it
    /// @dev Calls Ownable contract constructor
    constructor(UpgradeableMaster _mainContract) Ownable(msg.sender) public {
        mainContract = _mainContract;
        versionId = 0;
    }

    /// @notice Updated!! Adds a new upgradeable contract to the list of contracts managed by the gatekeeper.
    /// @param uint8 contract_type 1  signals new governance contract, 2 new verifier and 3 new
    /// @param bytes32 process_type is zero for contract_type 1 and 2 and mapps a zksynch contract to a specific business process aka set of constraints
    /// @param addr Address of upgradeable contract to add
    function addUpgradeable(uint8 contract_type, bytes32 process_type, address addr) external {
        requireMaster(msg.sender);
        require (contract_type < 4);
        require(upgradeStatus == UpgradeStatus.Idle, "apc11"); /// apc11 - upgradeable contract can't be added during upgrade
        
        if (contract_type = 1) {
           managedContracts.governance = addr; 
        }

        if (contract_type = 2) {
           managedContracts.verifier = addr; 
        }

        if (contract_type = 3) {
            managedContracts.Process_type.push(process_type);
            managedContracts.zksynchcontracts.push(addr);
        }

        ///@notice OLD! managedContracts.push(Upgradeable(addr));
        emit NewUpgradable(versionId, addr);
    }

    /// @notice Update!! Starts upgrade (activates notice period)
    /// @param newTargets New managed contracts targets (if element of this array is equal to zero address it means that appropriate upgradeable contract wouldn't be upgraded this time)
    function startUpgrade(managedContracts calldata newTargets) external {
        requireMaster(msg.sender);
        require(upgradeStatus == UpgradeStatus.Idle, "spu11"); // spu11 - unable to activate active upgrade mode
 /// No longer required    require(newTargets.length == managedContracts.length, "spu12"); // spu12 - number of new targets must be equal to the number of managed contracts

        uint noticePeriod = mainContract.getNoticePeriod();
        mainContract.upgradeNoticePeriodStarted();
        upgradeStatus = UpgradeStatus.NoticePeriod;
        noticePeriodFinishTimestamp = now.add(noticePeriod);
/// no longer required        nextTargets = newTargets;
        nextTargets.governance = newTargets.governance;
        nextTargets.verifier = newTargets.verifier;
        for (uint i = 0; i <= newTargets.Process_type.length-1; i++) {
            nextTargets.Process_type.push(newTargets.Process_type[i]);
            nextTargets.zksynchcontracts.push(newTargets.zksynchcontracts[i]);
        }
        
        emit NoticePeriodStart(versionId, newTargets, noticePeriod);
    }

    /// @notice Cancels upgrade
    function cancelUpgrade() external {
        requireMaster(msg.sender);
        require(upgradeStatus != UpgradeStatus.Idle, "cpu11"); // cpu11 - unable to cancel not active upgrade mode

        mainContract.upgradeCanceled();
        upgradeStatus = UpgradeStatus.Idle;
        noticePeriodFinishTimestamp = 0;
        delete nextTargets;
        emit UpgradeCancel(versionId);
    }

    /// @notice Activates preparation status
    /// @return Bool flag indicating that preparation status has been successfully activated
    function startPreparation() external returns (bool) {
        requireMaster(msg.sender);
        require(upgradeStatus == UpgradeStatus.NoticePeriod, "ugp11"); // ugp11 - unable to activate preparation status in case of not active notice period status

        if (now >= noticePeriodFinishTimestamp) {
            upgradeStatus = UpgradeStatus.Preparation;
            mainContract.upgradePreparationStarted();
            emit PreparationStart(versionId);
            return true;
        } else {
            return false;
        }
    }

    /// @notice Updated!! Finishes upgrade
    /// @param targetsUpgradeParameters New targets upgrade parameters per each upgradeable contract
    function finishUpgrade(bytes[] calldata targetsUpgradeParameters) external {
        requireMaster(msg.sender);
        require(upgradeStatus == UpgradeStatus.Preparation, "fpu11"); // fpu11 - unable to finish upgrade without preparation status active
    ///    require(targetsUpgradeParameters.length == managedContracts.length, "fpu12"); // fpu12 - number of new targets upgrade parameters must be equal to the number of managed contracts
        require(mainContract.isReadyForUpgrade(), "fpu13"); // fpu13 - main contract is not ready for upgrade
        mainContract.upgradeFinishes();

        address newTarget = managedContracts.governance;
            if (newTarget != address(0)) {
                managedContracts.governance.upgradeTarget(newTarget, targetsUpgradeParameters[0]);
            }

        address newTarget = managedContracts.verifier;
            if (newTarget != address(0)) {
                managedContracts.verifier.upgradeTarget(newTarget, targetsUpgradeParameters[1]);
            }

        for (uint64 i = 2; i < managedContracts.zksynchcontracts.length; i++) {
            address newTarget = nextTargets.zksynchcontracts[i-2];
            if (newTarget != address(0)) {
                managedContracts.zksynchcontracts[i-2].upgradeTarget(newTarget, targetsUpgradeParameters[i]);
            }
        }
        versionId++;
        emit UpgradeComplete(versionId, nextTargets);

        upgradeStatus = UpgradeStatus.Idle;
        noticePeriodFinishTimestamp = 0;
        delete nextTargets;
    }

}
