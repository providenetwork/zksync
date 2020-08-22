pragma solidity ^0.5.0;


/// @title Interface of the upgradeable master contract (defines notice period duration and allows finish upgrade during preparation of it)
/// @author Matter Labs
interface UpgradeableMaster {

    /// @notice Notice period before activation preparation status of upgrade mode
    function getNoticePeriod(bytes32 process_type) external returns (uint);

    /// @notice Notifies contract that notice period started
    function upgradeNoticePeriodStarted(bytes32 process_type) external;

    /// @notice Notifies contract that upgrade preparation status is activated
    function upgradePreparationStarted(bytes32 process_type) external;

    /// @notice Notifies contract that upgrade canceled
    function upgradeCanceled(bytes32 process_type) external;

    /// @notice Notifies contract that upgrade finishes
    function upgradeFinishes(bytes32 process_type) external;

    /// @notice Checks that contract is ready for upgrade
    /// @return bool flag indicating that contract is ready for upgrade
    function isReadyForUpgrade(bytes32 process_type) external returns (bool);

}
