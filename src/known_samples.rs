#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleClass {
    Inferable,
    PackageVariant,
    NotPresent,
}

#[derive(Clone, Copy, Debug)]
pub struct KnownSample {
    pub appid: &'static str,
    pub display_name: &'static str,
    pub package_name: Option<&'static str>,
    pub class: SampleClass,
}

pub const KNOWN_SAMPLES: &[KnownSample] = &[
    KnownSample {
        appid: "wx26a31270d9ab25e0",
        display_name: "盒马鲜生",
        package_name: Some("盒马鲜生"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wx734c1ad7b3562129",
        display_name: "大众点评",
        package_name: Some("大众点评"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wxd947200f82267e58",
        display_name: "问卷星",
        package_name: Some("问卷星"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wx1131e5c71e668b5d",
        display_name: "前程无忧",
        package_name: Some("前程无忧"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wx4b9859057c6a6245",
        display_name: "九号电动共享钥匙",
        package_name: Some("九号电动共享钥匙"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wx5db79bd23a923e8e",
        display_name: "草料二维码",
        package_name: Some("草料二维码"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wx765e46cdab92a0df",
        display_name: "探鱼烤鱼",
        package_name: Some("探鱼烤鱼"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wx8dfb37eb0b9e1281",
        display_name: "投票助手",
        package_name: None,
        class: SampleClass::NotPresent,
    },
    KnownSample {
        appid: "wxaa0ad9faf645128f",
        display_name: "乐开门",
        package_name: Some("乐开门"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wx8fa711da001ba25e",
        display_name: "环岛中港通官方",
        package_name: Some("环岛中港通"),
        class: SampleClass::PackageVariant,
    },
    KnownSample {
        appid: "wx4eff699c2e813ab6",
        display_name: "腾讯微证券",
        package_name: Some("腾讯微证券"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wx82d43fee89cdc7df",
        display_name: "粤省事",
        package_name: Some("粤省事"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wxd4185d00bf7e08ac",
        display_name: "顺丰速运+",
        package_name: Some("顺丰速运+"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wx89a110bbf29698eb",
        display_name: "CoCo点单+",
        package_name: Some("CoCo都可点单+"),
        class: SampleClass::PackageVariant,
    },
    KnownSample {
        appid: "wx7220890401646b56",
        display_name: "九号二手车",
        package_name: Some("九号二手车"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wxd460de0ce8b6dcbb",
        display_name: "九号官方商城",
        package_name: Some("九号小程序"),
        class: SampleClass::PackageVariant,
    },
    KnownSample {
        appid: "wx7ce5b8ce4b97b06b",
        display_name: "小萝卜报名",
        package_name: Some("小萝卜报名"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wxaf35009675aa0b2a",
        display_name: "滴滴出行",
        package_name: Some("滴滴出行"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wx7523c9b73699af04",
        display_name: "拉钩招聘",
        package_name: Some("拉勾网"),
        class: SampleClass::PackageVariant,
    },
    KnownSample {
        appid: "wx207e0dca59f0b3bd",
        display_name: "小二直租",
        package_name: Some("小二直租"),
        class: SampleClass::Inferable,
    },
    KnownSample {
        appid: "wxa58bebfaaccc254b",
        display_name: "高州智慧泊车",
        package_name: Some("高州智慧泊车"),
        class: SampleClass::Inferable,
    },
];
